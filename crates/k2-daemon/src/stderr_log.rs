//! The daemon's own log files stay bounded.
//!
//! The daemon logs to stderr (`k2_core::log_debug!`). Its service manager
//! points stderr and stdout at files: launchd's `StandardErrorPath` on
//! macOS and systemd's `StandardError=append:` on Linux both name
//! `~/.k2/daemon.stderr.log` (and `daemon.stdout.log`). Neither rotates
//! them, so the file only grew (one Mac's reached 653 MB while its disk
//! filled).
//!
//! Now the daemon rotates them itself. At start and every
//! [`CHECK_EVERY`] it looks at fd 2 (and fd 1): when it is the file at
//! that path (same device and inode) and has reached [`ROTATE_AT`], the
//! file moves to `<name>.1` (`.1` → `.2`, `.2` → `.3`; the oldest is
//! deleted), a fresh file is opened at the path and put over the fd
//! (`dup2`). A file that came in far over the cap (an older daemon's) is
//! cut to its last [`ROTATE_AT`] bytes once it is `.1`, so the total stays
//! near `(KEEP + 1) × ROTATE_AT`.
//!
//! When the fd is anything else (a terminal, a pipe, a test harness's
//! file, Windows), nothing is touched.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A log file this size or larger is rotated.
pub const ROTATE_AT: u64 = 50 * 1024 * 1024;
/// Old files kept: `<name>.1` … `<name>.3`.
pub const KEEP: u32 = 3;
/// How often the size is looked at.
pub const CHECK_EVERY: Duration = Duration::from_secs(30);

/// `<path>.<n>`.
pub fn generation(path: &Path, n: u32) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(format!(".{n}"));
    PathBuf::from(s)
}

/// Move `path` to `path.1`, after `path.1..path.(keep-1)` each move up one
/// and `path.keep` is deleted. A missing generation is skipped.
pub fn shift_generations(path: &Path, keep: u32) -> io::Result<()> {
    if keep == 0 {
        return match fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    match fs::remove_file(generation(path, keep)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    for n in (1..keep).rev() {
        match fs::rename(generation(path, n), generation(path, n + 1)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    fs::rename(path, generation(path, 1))
}

/// Keep only the last `cap` bytes of `path` (a closed, rotated file).
/// Written to a sibling first and renamed over, so a failure (a full disk)
/// leaves the file as it was.
pub fn trim_to_tail(path: &Path, cap: u64) -> io::Result<()> {
    let mut src = File::open(path)?;
    let len = src.metadata()?.len();
    if len <= cap {
        return Ok(());
    }
    src.seek(SeekFrom::Start(len - cap))?;
    let mut tmp_name = path.as_os_str().to_os_string();
    tmp_name.push(".trim");
    let tmp = PathBuf::from(tmp_name);
    let result = (|| {
        let mut dst = File::create(&tmp)?;
        dst.write_all(b"[earlier lines were cut when this log was rotated]\n")?;
        io::copy(&mut (&mut src).take(cap), &mut dst)?;
        dst.flush()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(unix)]
mod unix_impl {
    use super::*;
    use std::fs::OpenOptions;
    use std::mem::ManuallyDrop;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};

    /// The metadata of the file open on `fd` (the fd is not closed).
    fn fd_metadata(fd: RawFd) -> io::Result<fs::Metadata> {
        // SAFETY: the File is never dropped (ManuallyDrop), so the fd stays
        // open and owned by its caller; it is only fstat-ed.
        let f = ManuallyDrop::new(unsafe { File::from_raw_fd(fd) });
        f.metadata()
    }

    /// Rotate the log at `path` when `fd` is that very file and has reached
    /// `cap` bytes; the fd then writes to a fresh file at `path`. Returns
    /// whether it rotated.
    pub fn rotate_fd_if_over(fd: RawFd, path: &Path, cap: u64, keep: u32) -> io::Result<bool> {
        let Ok(open) = fd_metadata(fd) else { return Ok(false) };
        let Ok(on_disk) = fs::metadata(path) else { return Ok(false) };
        if !open.is_file() || open.dev() != on_disk.dev() || open.ino() != on_disk.ino() {
            return Ok(false);
        }
        if open.len() < cap {
            return Ok(false);
        }
        shift_generations(path, keep)?;
        let fresh = OpenOptions::new().create(true).append(true).mode(0o600).open(path)?;
        // SAFETY: both fds are open; dup2 atomically points `fd` at the
        // fresh file (closing the old description's use by `fd`).
        if unsafe { libc::dup2(fresh.as_raw_fd(), fd) } < 0 {
            return Err(io::Error::last_os_error());
        }
        drop(fresh);
        if keep > 0 && open.len() > cap.saturating_mul(2) {
            let _ = trim_to_tail(&generation(path, 1), cap);
        }
        Ok(true)
    }

    /// Start the rotation thread (the daemon's boot). Checks once at once.
    pub fn start() {
        let home = k2_core::paths::k2_home();
        let logs = [(2, home.join("daemon.stderr.log")), (1, home.join("daemon.stdout.log"))];
        let check = move || {
            for (fd, path) in &logs {
                match rotate_fd_if_over(*fd, path, ROTATE_AT, KEEP) {
                    Ok(true) => k2_core::log_debug!("[daemon/log] rotated {} (over {} MB)", path.display(), ROTATE_AT / (1024 * 1024)),
                    Ok(false) => {}
                    Err(e) => k2_core::log_debug!("[daemon/log] could not rotate {}: {e}", path.display()),
                }
            }
        };
        let _ = std::thread::Builder::new().name("daemon-log-rotate".into()).spawn(move || loop {
            check();
            std::thread::sleep(CHECK_EVERY);
        });
    }
}

#[cfg(unix)]
pub use unix_impl::{rotate_fd_if_over, start};

/// Windows: the daemon's stderr is not a service-manager file; nothing to do.
#[cfg(not(unix))]
pub fn start() {}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::os::unix::io::{AsRawFd, FromRawFd};

    /// A fresh directory under the system temp dir, removed on drop.
    struct Dir(PathBuf);
    impl Dir {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn tempdir() -> Dir {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "k2-stderr-log-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).expect("temp dir");
        Dir(p)
    }

    fn read(p: &Path) -> String {
        fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }

    /// An fd of our own pointing at `path` in append mode (never fd 1 or 2).
    fn log_fd(path: &Path) -> File {
        let f = OpenOptions::new().create(true).append(true).open(path).expect("open log");
        // SAFETY: dup of an open fd; the new fd is owned by the File returned.
        let fd = unsafe { libc::dup(f.as_raw_fd()) };
        assert!(fd > 2, "dup gave {fd}");
        unsafe { File::from_raw_fd(fd) }
    }

    #[test]
    fn under_the_cap_nothing_moves() {
        let dir = tempdir();
        let path = dir.path().join("daemon.stderr.log");
        let mut f = log_fd(&path);
        f.write_all(b"short\n").expect("write");
        assert!(!rotate_fd_if_over(f.as_raw_fd(), &path, 100, 3).expect("check"));
        assert_eq!(read(&path), "short\n");
        assert!(!generation(&path, 1).exists());
    }

    #[test]
    fn over_the_cap_the_fd_writes_to_a_fresh_file_and_three_old_ones_are_kept() {
        let dir = tempdir();
        let path = dir.path().join("daemon.stderr.log");
        let mut f = log_fd(&path);
        for round in 1..=5 {
            f.write_all(format!("round {round} {}\n", "x".repeat(120)).as_bytes()).expect("write");
            assert!(rotate_fd_if_over(f.as_raw_fd(), &path, 100, 3).expect("rotate"), "round {round}");
            // The same fd now writes into the new file at the path.
            f.write_all(format!("after {round}\n").as_bytes()).expect("write after");
            assert_eq!(read(&path), format!("after {round}\n"), "round {round}");
        }
        // Newest first: .1 holds round 5's lines (plus round 4's "after").
        assert!(read(&generation(&path, 1)).contains("round 5"), "{}", read(&generation(&path, 1)));
        assert!(read(&generation(&path, 2)).contains("round 4"));
        assert!(read(&generation(&path, 3)).contains("round 3"));
        assert!(!generation(&path, 4).exists(), "only 3 old files are kept");
        let names: Vec<String> = fs::read_dir(dir.path())
            .expect("ls")
            .map(|e| e.expect("entry").file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names.len(), 4, "{names:?}");
    }

    #[test]
    fn an_fd_that_is_not_the_file_at_the_path_is_left_alone() {
        let dir = tempdir();
        let path = dir.path().join("daemon.stderr.log");
        fs::write(&path, "x".repeat(500)).expect("seed");
        let other = dir.path().join("other.log");
        let mut f = log_fd(&other);
        f.write_all("y".repeat(500).as_bytes()).expect("write");
        assert!(!rotate_fd_if_over(f.as_raw_fd(), &path, 100, 3).expect("check"));
        assert_eq!(read(&path).len(), 500);
        assert!(!generation(&path, 1).exists());
        // And a missing log path is no error.
        assert!(!rotate_fd_if_over(f.as_raw_fd(), &dir.path().join("none.log"), 100, 3).expect("missing"));
    }

    #[test]
    fn a_file_far_over_the_cap_is_cut_to_its_tail_when_rotated() {
        let dir = tempdir();
        let path = dir.path().join("daemon.stderr.log");
        let mut f = log_fd(&path);
        let body: String = (0..100).map(|i| format!("line {i:03}\n")).collect();
        f.write_all(body.as_bytes()).expect("write");
        assert!(body.len() > 2 * 200);
        assert!(rotate_fd_if_over(f.as_raw_fd(), &path, 200, 3).expect("rotate"));
        let old = read(&generation(&path, 1));
        assert!(old.starts_with("[earlier lines were cut when this log was rotated]\n"), "{old}");
        assert!(old.ends_with("line 099\n"), "{old}");
        assert!(old.len() <= 200 + 60, "{}", old.len());
        assert!(!dir.path().join("daemon.stderr.log.1.trim").exists());
    }

    #[test]
    fn shift_with_gaps_and_keep_zero() {
        let dir = tempdir();
        let path = dir.path().join("a.log");
        fs::write(&path, "now").expect("w");
        fs::write(generation(&path, 2), "two").expect("w");
        shift_generations(&path, 3).expect("shift");
        assert_eq!(read(&generation(&path, 1)), "now");
        assert_eq!(read(&generation(&path, 3)), "two");
        assert!(!path.exists());
        fs::write(&path, "again").expect("w");
        shift_generations(&path, 0).expect("keep 0");
        assert!(!path.exists());
    }
}
