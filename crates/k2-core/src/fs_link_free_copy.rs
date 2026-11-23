//! A folder copy that never follows a link (the App guest copy and move
//! doors, `fs_commands::copy_files_no_follow` / `move_files_no_follow`).
//!
//! Unix: every entry is looked at with `fstatat(AT_SYMLINK_NOFOLLOW)` and
//! opened with `openat` relative to its folder's open descriptor, with
//! `O_NOFOLLOW`, so a name swapped for a link between the look and the open
//! fails instead of being followed. Folders on the destination side are
//! made with `mkdirat` and opened the same way, and files are created with
//! `O_CREAT|O_EXCL|O_NOFOLLOW`.
//!
//! What the walk copies: plain folders, and plain files with one name (one
//! hard link). Everything else (a symbolic link, a file with other hard
//! links, a pipe, a socket, a device) is left out under
//! [`OtherEntry::Skip`] (the copy door: it counts them) or stops the walk
//! under [`OtherEntry::Refuse`] (a move that falls back to copy-then-delete:
//! the source must not lose them). A refused walk removes the folder it
//! made. Windows: the same rules with `symlink_metadata` and
//! `FILE_FLAG_OPEN_REPARSE_POINT` (a reparse point counts as a link).

use std::fs;
use std::path::Path;

/// What the walk does with an entry that is not a plain folder or a plain
/// one-name file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OtherEntry {
    /// Leave it out of the copy and count it.
    Skip,
    /// Stop: nothing past it is copied and the new folder is removed.
    Refuse,
}

/// The error text for an entry [`OtherEntry::Refuse`] stops at.
fn refused_text(rel: &Path) -> String {
    format!(
        "{} is a link or not a plain file, so it can't be copied",
        rel.display()
    )
}

/// What a copy wrote and what it left out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Files and folders made in the copy (the top folder included).
    pub copied: u64,
    /// Entries left out under [`OtherEntry::Skip`].
    pub skipped: u64,
}

/// Copy the folder `src` to the NEW folder `dst` (it must not exist; its
/// parent must). Returns what was copied and left out ([`OtherEntry::Skip`]).
/// On an error the folder made at `dst` is removed.
pub fn copy_dir(src: &Path, dst: &Path, other: OtherEntry) -> Result<Counts, String> {
    let mut count = Counts::default();
    let made = imp::copy_dir(src, dst, other, &mut count);
    match made {
        Ok(()) => Ok(count),
        Err((e, created)) => {
            if created {
                // `remove_dir_all` never follows a link it meets.
                if let Err(rm) = fs::remove_dir_all(dst) {
                    return Err(format!("{e} (and the partial copy at {} stays: {rm})", dst.display()));
                }
            }
            Err(e)
        }
    }
}

/// Copy the plain file `src` to the NEW file `dst`. A link, a file with
/// other hard links, or anything that isn't a plain file is refused.
pub fn copy_file(src: &Path, dst: &Path) -> Result<(), String> {
    let name = src.file_name().map(Path::new).unwrap_or(src);
    imp::copy_file(src, dst, name)
}

#[cfg(unix)]
mod imp {
    use super::{refused_text, Counts, OtherEntry};
    use std::collections::HashSet;
    use std::ffi::{CStr, CString, OsString};
    use std::fs::File;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};

    type Walk<T> = Result<T, (String, bool)>;

    fn c_path(p: &Path) -> io::Result<CString> {
        CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
    }

    fn check(rc: libc::c_int) -> io::Result<libc::c_int> {
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(rc)
        }
    }

    fn owned(fd: libc::c_int) -> io::Result<OwnedFd> {
        // SAFETY: `fd` was just returned by a successful open call and is
        // owned by nobody else.
        check(fd).map(|fd| unsafe { OwnedFd::from_raw_fd(fd) })
    }

    const DIR_FLAGS: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

    fn open_dir(path: &Path) -> io::Result<OwnedFd> {
        let c = c_path(path)?;
        // SAFETY: `c` is a valid NUL-terminated string.
        owned(unsafe { libc::open(c.as_ptr(), DIR_FLAGS) })
    }

    /// A destination's (or a source file's) parent folder, as the room
    /// checks resolved it: a link on the way is followed here, once.
    fn open_parent(path: &Path) -> io::Result<OwnedFd> {
        let c = c_path(path)?;
        // SAFETY: `c` is a valid NUL-terminated string.
        owned(unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) })
    }

    fn open_dir_at(dir: RawFd, name: &CStr) -> io::Result<OwnedFd> {
        // SAFETY: `dir` is an open descriptor, `name` a valid C string.
        owned(unsafe { libc::openat(dir, name.as_ptr(), DIR_FLAGS) })
    }

    fn lstat_at(dir: RawFd, name: &CStr) -> io::Result<libc::stat> {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: as above; `st` is a valid out pointer.
        check(unsafe { libc::fstatat(dir, name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) })?;
        Ok(st)
    }

    fn fstat(fd: RawFd) -> io::Result<libc::stat> {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: `fd` is open; `st` is a valid out pointer.
        check(unsafe { libc::fstat(fd, &mut st) })?;
        Ok(st)
    }

    // `dev_t` is i32 on macOS and u64 on Linux.
    #[allow(clippy::unnecessary_cast)]
    fn id(st: &libc::stat) -> (u64, u64) {
        (st.st_dev as u64, st.st_ino as u64)
    }

    fn mode_bits(st: &libc::stat) -> libc::mode_t {
        st.st_mode & 0o777
    }

    fn is_kind(st: &libc::stat, kind: libc::mode_t) -> bool {
        st.st_mode & libc::S_IFMT == kind
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn clear_errno() {
        // SAFETY: the thread's errno slot.
        unsafe { *libc::__errno_location() = 0 }
    }

    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    fn clear_errno() {
        // SAFETY: the thread's errno slot.
        unsafe { *libc::__error() = 0 }
    }

    #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos", target_os = "ios", target_os = "freebsd")))]
    fn clear_errno() {}

    /// The names in the open folder `dir` (not `.` / `..`), read through a
    /// duplicate of its descriptor.
    fn list(dir: RawFd) -> io::Result<Vec<OsString>> {
        // SAFETY: `dir` is open; F_DUPFD_CLOEXEC makes a new descriptor.
        let dup = check(unsafe { libc::fcntl(dir, libc::F_DUPFD_CLOEXEC, 0) })?;
        // SAFETY: `dup` is ours; fdopendir takes it over on success.
        let stream = unsafe { libc::fdopendir(dup) };
        if stream.is_null() {
            let e = io::Error::last_os_error();
            // SAFETY: fdopendir failed, so `dup` is still ours to close.
            unsafe { libc::close(dup) };
            return Err(e);
        }
        let mut names = Vec::new();
        let result = loop {
            clear_errno();
            // SAFETY: `stream` is a valid DIR*.
            let ent = unsafe { libc::readdir(stream) };
            if ent.is_null() {
                let e = io::Error::last_os_error();
                break match e.raw_os_error() {
                    Some(0) | None => Ok(()),
                    Some(_) => Err(e),
                };
            }
            // SAFETY: `d_name` is a NUL-terminated name inside `*ent`.
            let name = unsafe { CStr::from_ptr((*ent).d_name.as_ptr()) };
            let bytes = name.to_bytes();
            if bytes != b"." && bytes != b".." {
                names.push(OsString::from_vec(bytes.to_vec()));
            }
        };
        // SAFETY: `stream` is valid; this also closes `dup`.
        unsafe { libc::closedir(stream) };
        result.map(|()| names)
    }

    /// Make the folder `name` in `dir` and open it (no-follow).
    fn make_dir_at(dir: RawFd, name: &CStr) -> io::Result<OwnedFd> {
        // SAFETY: `dir` is open, `name` a valid C string.
        check(unsafe { libc::mkdirat(dir, name.as_ptr(), 0o700) })?;
        open_dir_at(dir, name)
    }

    /// Copy the plain file opened as `src` into a new file `name` in `dst`.
    // `mode_t` is u16 on macOS and u32 on Linux (openat takes a c_uint).
    #[allow(clippy::unnecessary_cast)]
    fn copy_open_file(src: &mut File, dst_dir: RawFd, name: &CStr, mode: libc::mode_t) -> io::Result<()> {
        let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        // SAFETY: `dst_dir` is open, `name` a valid C string.
        let fd = owned(unsafe { libc::openat(dst_dir, name.as_ptr(), flags, mode as libc::c_uint) })?;
        let mut out = File::from(fd);
        io::copy(src, &mut out)?;
        Ok(())
    }

    /// Open `name` in `dir` for reading, no-follow and non-blocking (a pipe
    /// never blocks the open), and confirm on the open file that it is a
    /// plain file with one name.
    fn open_plain_at(dir: RawFd, name: &CStr) -> io::Result<Option<(File, libc::stat)>> {
        let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
        // SAFETY: `dir` is open, `name` a valid C string.
        let fd = match owned(unsafe { libc::openat(dir, name.as_ptr(), flags) }) {
            Ok(fd) => fd,
            // A link swapped in after the look: O_NOFOLLOW refuses it.
            Err(e) if e.raw_os_error() == Some(libc::ELOOP) => return Ok(None),
            Err(e) => return Err(e),
        };
        let st = fstat(fd.as_raw_fd())?;
        if !is_kind(&st, libc::S_IFREG) || st.st_nlink != 1 {
            return Ok(None);
        }
        Ok(Some((File::from(fd), st)))
    }

    fn walk(
        src: RawFd,
        dst: RawFd,
        rel: &Path,
        other: OtherEntry,
        made: &mut HashSet<(u64, u64)>,
        count: &mut Counts,
    ) -> Result<(), String> {
        let names = list(src).map_err(|e| format!("Failed to read directory {}: {e}", rel.display()))?;
        for name in names {
            let rel_name = rel.join(&name);
            let c = CString::new(name.into_vec()).map_err(|_| format!("Invalid name: {}", rel_name.display()))?;
            let st = match lstat_at(src, &c) {
                Ok(st) => st,
                // Gone since the listing: nothing to copy.
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("Failed to read {}: {e}", rel_name.display())),
            };
            if is_kind(&st, libc::S_IFDIR) {
                // The copy being made, when it sits inside its own source.
                if made.contains(&id(&st)) {
                    continue;
                }
                let s = match open_dir_at(src, &c) {
                    Ok(fd) => fd,
                    Err(e) if e.raw_os_error() == Some(libc::ELOOP) || e.raw_os_error() == Some(libc::ENOTDIR) => {
                        other_entry(&rel_name, other, count)?;
                        continue;
                    }
                    Err(e) => return Err(format!("Failed to open {}: {e}", rel_name.display())),
                };
                let d = make_dir_at(dst, &c).map_err(|e| format!("Failed to create directory {}: {e}", rel_name.display()))?;
                let dst_st = fstat(d.as_raw_fd()).map_err(|e| format!("Failed to read {}: {e}", rel_name.display()))?;
                made.insert(id(&dst_st));
                count.copied += 1;
                walk(s.as_raw_fd(), d.as_raw_fd(), &rel_name, other, made, count)?;
                // SAFETY: `d` is an open folder descriptor.
                check(unsafe { libc::fchmod(d.as_raw_fd(), mode_bits(&st)) })
                    .map_err(|e| format!("Failed to set the mode of {}: {e}", rel_name.display()))?;
            } else if is_kind(&st, libc::S_IFREG) && st.st_nlink == 1 {
                match open_plain_at(src, &c).map_err(|e| format!("Failed to open {}: {e}", rel_name.display()))? {
                    Some((mut f, fst)) => {
                        copy_open_file(&mut f, dst, &c, mode_bits(&fst))
                            .map_err(|e| format!("Failed to copy {}: {e}", rel_name.display()))?;
                        count.copied += 1;
                    }
                    None => other_entry(&rel_name, other, count)?,
                }
            } else {
                other_entry(&rel_name, other, count)?;
            }
        }
        Ok(())
    }

    fn other_entry(rel: &Path, other: OtherEntry, count: &mut Counts) -> Result<(), String> {
        match other {
            OtherEntry::Skip => {
                count.skipped += 1;
                Ok(())
            }
            OtherEntry::Refuse => Err(refused_text(rel)),
        }
    }

    /// `(error, made)`: `made` is set once the destination folder exists.
    pub(super) fn copy_dir(src: &Path, dst: &Path, other: OtherEntry, count: &mut Counts) -> Walk<()> {
        let src_name = PathBuf::from(src.file_name().unwrap_or(src.as_os_str()));
        let s = open_dir(src).map_err(|e| match e.raw_os_error() {
            Some(libc::ELOOP) | Some(libc::ENOTDIR) => (refused_text(&src_name), false),
            _ => (format!("Failed to read directory: {e}"), false),
        })?;
        let src_st = fstat(s.as_raw_fd()).map_err(|e| (format!("Failed to read directory: {e}"), false))?;
        let (parent, name) = match (dst.parent(), dst.file_name()) {
            (Some(p), Some(n)) => (p, n),
            _ => return Err((format!("Invalid destination: {}", dst.display()), false)),
        };
        let p = open_parent(parent).map_err(|e| (format!("Failed to open the destination folder: {e}"), false))?;
        let c = CString::new(name.as_bytes()).map_err(|_| ("Invalid destination name".to_string(), false))?;
        let d = make_dir_at(p.as_raw_fd(), &c).map_err(|e| (format!("Failed to create directory: {e}"), false))?;
        let mut made = HashSet::new();
        let dst_st = fstat(d.as_raw_fd()).map_err(|e| (format!("Failed to read directory: {e}"), true))?;
        made.insert(id(&dst_st));
        count.copied += 1;
        walk(s.as_raw_fd(), d.as_raw_fd(), &src_name, other, &mut made, count).map_err(|e| (e, true))?;
        // SAFETY: `d` is an open folder descriptor.
        check(unsafe { libc::fchmod(d.as_raw_fd(), mode_bits(&src_st)) })
            .map_err(|e| (format!("Failed to set the mode of the copy: {e}"), true))?;
        Ok(())
    }

    pub(super) fn copy_file(src: &Path, dst: &Path, shown: &Path) -> Result<(), String> {
        let (sp, sn) = match (src.parent(), src.file_name()) {
            (Some(p), Some(n)) => (p, n),
            _ => return Err(format!("Invalid source: {}", src.display())),
        };
        let (dp, dn) = match (dst.parent(), dst.file_name()) {
            (Some(p), Some(n)) => (p, n),
            _ => return Err(format!("Invalid destination: {}", dst.display())),
        };
        let sdir = open_parent(sp).map_err(|e| format!("Failed to copy: {e}"))?;
        let ddir = open_parent(dp).map_err(|e| format!("Failed to copy: {e}"))?;
        let sc = CString::new(sn.as_bytes()).map_err(|_| "Invalid source name".to_string())?;
        let dc = CString::new(dn.as_bytes()).map_err(|_| "Invalid destination name".to_string())?;
        match open_plain_at(sdir.as_raw_fd(), &sc).map_err(|e| format!("Failed to copy: {e}"))? {
            Some((mut f, st)) => copy_open_file(&mut f, ddir.as_raw_fd(), &dc, mode_bits(&st))
                .map_err(|e| format!("Failed to copy: {e}")),
            None => Err(refused_text(shown)),
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::{refused_text, Counts, OtherEntry};
    use std::collections::HashSet;
    use std::fs::{self, File, OpenOptions};
    use std::io;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    type Walk<T> = Result<T, (String, bool)>;

    fn is_link(md: &fs::Metadata) -> bool {
        md.file_type().is_symlink() || md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    /// Open `path` for reading without following a reparse point, and
    /// confirm on the open file that it is a plain file.
    fn open_plain(path: &Path) -> io::Result<Option<File>> {
        let f = OpenOptions::new().read(true).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(path)?;
        let md = f.metadata()?;
        if !md.is_file() || is_link(&md) {
            return Ok(None);
        }
        Ok(Some(f))
    }

    fn copy_open_file(src: &mut File, dst: &Path) -> io::Result<()> {
        let mut out = OpenOptions::new().write(true).create_new(true).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(dst)?;
        io::copy(src, &mut out)?;
        Ok(())
    }

    fn other_entry(rel: &Path, other: OtherEntry, count: &mut Counts) -> Result<(), String> {
        match other {
            OtherEntry::Skip => {
                count.skipped += 1;
                Ok(())
            }
            OtherEntry::Refuse => Err(refused_text(rel)),
        }
    }

    fn walk(src: &Path, dst: &Path, rel: &Path, other: OtherEntry, made: &mut HashSet<PathBuf>, count: &mut Counts) -> Result<(), String> {
        let mut names = Vec::new();
        for entry in fs::read_dir(src).map_err(|e| format!("Failed to read directory {}: {e}", rel.display()))? {
            names.push(entry.map_err(|e| format!("Failed to read entry: {e}"))?.file_name());
        }
        for name in names {
            let (s, d, rel_name) = (src.join(&name), dst.join(&name), rel.join(&name));
            let md = match fs::symlink_metadata(&s) {
                Ok(md) => md,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("Failed to read {}: {e}", rel_name.display())),
            };
            if is_link(&md) {
                other_entry(&rel_name, other, count)?;
            } else if md.is_dir() {
                if made.contains(&s) {
                    continue;
                }
                fs::create_dir(&d).map_err(|e| format!("Failed to create directory {}: {e}", rel_name.display()))?;
                made.insert(d.clone());
                count.copied += 1;
                walk(&s, &d, &rel_name, other, made, count)?;
            } else if md.is_file() {
                match open_plain(&s).map_err(|e| format!("Failed to open {}: {e}", rel_name.display()))? {
                    Some(mut f) => {
                        copy_open_file(&mut f, &d).map_err(|e| format!("Failed to copy {}: {e}", rel_name.display()))?;
                        count.copied += 1;
                    }
                    None => other_entry(&rel_name, other, count)?,
                }
            } else {
                other_entry(&rel_name, other, count)?;
            }
        }
        Ok(())
    }

    pub(super) fn copy_dir(src: &Path, dst: &Path, other: OtherEntry, count: &mut Counts) -> Walk<()> {
        let src_name = PathBuf::from(src.file_name().unwrap_or(src.as_os_str()));
        let md = fs::symlink_metadata(src).map_err(|e| (format!("Failed to read directory: {e}"), false))?;
        if is_link(&md) || !md.is_dir() {
            return Err((refused_text(&src_name), false));
        }
        fs::create_dir(dst).map_err(|e| (format!("Failed to create directory: {e}"), false))?;
        let mut made = HashSet::new();
        made.insert(dst.to_path_buf());
        count.copied += 1;
        walk(src, dst, &src_name, other, &mut made, count).map_err(|e| (e, true))
    }

    pub(super) fn copy_file(src: &Path, dst: &Path, shown: &Path) -> Result<(), String> {
        let md = fs::symlink_metadata(src).map_err(|e| format!("Failed to copy: {e}"))?;
        if is_link(&md) || !md.is_file() {
            return Err(refused_text(shown));
        }
        match open_plain(src).map_err(|e| format!("Failed to copy: {e}"))? {
            Some(mut f) => copy_open_file(&mut f, dst).map_err(|e| format!("Failed to copy: {e}")),
            None => Err(refused_text(shown)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("k2-link-free-copy-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&d).expect("temp dir");
        dunce::canonicalize(&d).expect("canon temp")
    }

    /// A folder with a file, a sub-folder, a link to a file outside, a
    /// link to a folder outside and a link inside the folder.
    fn tree(tag: &str) -> (PathBuf, PathBuf) {
        let root = temp(tag);
        let outside = temp(&format!("{tag}-out"));
        fs::write(outside.join("other.txt"), b"bytes from outside").expect("outside file");
        fs::create_dir_all(outside.join("dir")).expect("outside dir");
        fs::write(outside.join("dir/inner.txt"), b"inner").expect("outside inner");
        let src = root.join("src");
        fs::create_dir_all(src.join("sub")).expect("sub");
        fs::write(src.join("a.txt"), b"a").expect("a");
        fs::write(src.join("sub/b.txt"), b"b").expect("b");
        crate::test_env::symlink(outside.join("other.txt"), src.join("file-link")).expect("file link");
        crate::test_env::symlink(outside.join("dir"), src.join("dir-link")).expect("dir link");
        crate::test_env::symlink(src.join("a.txt"), src.join("sub/near-link")).expect("near link");
        (root, outside)
    }

    #[test]
    fn skip_copies_plain_entries_and_counts_the_links_it_leaves_out() {
        let (root, outside) = tree("skip");
        let dst = root.join("copy");
        let count = copy_dir(&root.join("src"), &dst, OtherEntry::Skip).expect("copy");
        // src, a.txt, sub, sub/b.txt; three links left out.
        assert_eq!(count, Counts { copied: 4, skipped: 3 });
        assert_eq!(fs::read(dst.join("a.txt")).expect("a"), b"a");
        assert_eq!(fs::read(dst.join("sub/b.txt")).expect("b"), b"b");
        for name in ["file-link", "dir-link", "sub/near-link"] {
            assert!(fs::symlink_metadata(dst.join(name)).is_err(), "{name} must be left out");
        }
        fs::remove_dir_all(&root).expect("clean");
        fs::remove_dir_all(&outside).expect("clean outside");
    }

    #[test]
    fn refuse_stops_at_a_link_and_removes_the_partial_copy() {
        let (root, outside) = tree("refuse");
        let dst = root.join("copy");
        let err = copy_dir(&root.join("src"), &dst, OtherEntry::Refuse).expect_err("a link stops the walk");
        assert!(err.contains("is a link or not a plain file"), "{err}");
        assert!(fs::symlink_metadata(&dst).is_err(), "the partial copy is removed");
        assert_eq!(fs::read(outside.join("other.txt")).expect("outside"), b"bytes from outside");
        fs::remove_dir_all(&root).expect("clean");
        fs::remove_dir_all(&outside).expect("clean outside");
    }

    #[test]
    fn a_folder_copied_into_itself_copies_once() {
        let root = temp("self");
        fs::create_dir_all(root.join("a/sub")).expect("a/sub");
        fs::write(root.join("a/sub/f.txt"), b"f").expect("f");
        let count = copy_dir(&root.join("a"), &root.join("a/sub/a"), OtherEntry::Skip).expect("copy");
        assert_eq!(count, Counts { copied: 3, skipped: 0 });
        assert_eq!(fs::read(root.join("a/sub/a/sub/f.txt")).expect("copied"), b"f");
        assert!(fs::symlink_metadata(root.join("a/sub/a/sub/a")).is_err(), "the copy is not copied into itself");
        fs::remove_dir_all(&root).expect("clean");
    }

    #[test]
    fn a_file_copy_refuses_a_link_and_copies_a_plain_file() {
        let (root, outside) = tree("file");
        let err = copy_file(&root.join("src/file-link"), &root.join("got")).expect_err("a link");
        assert!(err.contains("file-link is a link or not a plain file"), "{err}");
        assert!(fs::symlink_metadata(root.join("got")).is_err());
        copy_file(&root.join("src/a.txt"), &root.join("got")).expect("a plain file");
        assert_eq!(fs::read(root.join("got")).expect("got"), b"a");
        fs::remove_dir_all(&root).expect("clean");
        fs::remove_dir_all(&outside).expect("clean outside");
    }

    #[cfg(unix)]
    #[test]
    fn a_file_with_other_hard_links_is_left_out() {
        let root = temp("hard");
        let outside = temp("hard-out");
        fs::write(outside.join("other.txt"), b"bytes from outside").expect("outside");
        fs::create_dir_all(root.join("src")).expect("src");
        fs::hard_link(outside.join("other.txt"), root.join("src/hard.txt")).expect("hard link");
        let count = copy_dir(&root.join("src"), &root.join("copy"), OtherEntry::Skip).expect("copy");
        assert_eq!(count, Counts { copied: 1, skipped: 1 });
        assert!(fs::symlink_metadata(root.join("copy/hard.txt")).is_err());
        let err = copy_file(&root.join("src/hard.txt"), &root.join("got")).expect_err("hard link");
        assert!(err.contains("is a link or not a plain file"), "{err}");
        fs::remove_dir_all(&root).expect("clean");
        fs::remove_dir_all(&outside).expect("clean outside");
    }
}
