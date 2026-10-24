//! Small shared helpers: time, atomic writes, logging, bounded commands.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Unix seconds now.
pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// One stderr line with a timestamp (launchd / journald capture it).
#[macro_export]
macro_rules! nlog {
    ($($arg:tt)*) => {{
        eprintln!("[k2-node {}] {}", $crate::util::now(), format!($($arg)*));
    }};
}

/// Write `bytes` to `path` via a temp file + rename, then chmod `mode`.
pub fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    atomic_write_owned(path, bytes, mode, None)
}

/// [`atomic_write`], and as root give the new file `owner` (uid, gid).
///
/// Safe in a directory other users can write (review A, H1/M5): the
/// parent is opened once (`O_DIRECTORY|O_NOFOLLOW`) and everything after
/// goes through that fd. The temp file gets a random name and is created
/// `O_CREAT|O_EXCL|O_NOFOLLOW` (a planted file or link at that name
/// fails it; it never truncates or follows anything); its mode and owner
/// are set on its fd; `renameat` replaces whatever `path` was (a link is
/// replaced, never followed). As root, the parent must be root's: root
/// never writes into a directory another user owns (the node's home).
pub fn atomic_write_owned(path: &Path, bytes: &[u8], mode: u32, owner: Option<(u32, u32)>) -> Result<(), String> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let dir = path.parent().ok_or_else(|| format!("{} has no parent", path.display()))?;
    let name = path.file_name().ok_or_else(|| format!("{} has no file name", path.display()))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let cdir = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()).map_err(|_| format!("{}: a NUL in the path", dir.display()))?;
    // SAFETY: a valid C string; the fd is owned below.
    let dfd = unsafe { libc::open(cdir.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if dfd < 0 {
        return Err(format!("open {}: {}", dir.display(), std::io::Error::last_os_error()));
    }
    // SAFETY: a fresh fd nothing else owns.
    let dfd = unsafe { OwnedFd::from_raw_fd(dfd) };
    // SAFETY: plain getter.
    if unsafe { libc::geteuid() } == 0 {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: a valid fd and out-pointer.
        if unsafe { libc::fstat(dfd.as_raw_fd(), &mut st) } != 0 {
            return Err(format!("stat {}: {}", dir.display(), std::io::Error::last_os_error()));
        }
        if st.st_uid != 0 {
            return Err(format!("root never writes into {} (owned by uid {})", dir.display(), st.st_uid));
        }
    }
    let base = name.to_string_lossy();
    let mut made = None;
    for _ in 0..8 {
        let rand = k2_node_proto::crypto::random_id()?;
        let tmp = std::ffi::CString::new(format!(".{base}.tmp-{}", &rand[..16])).map_err(|_| format!("{}: a NUL in the name", path.display()))?;
        // SAFETY: a valid dir fd and C string.
        let fd = unsafe { libc::openat(dfd.as_raw_fd(), tmp.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0o600 as libc::c_uint) };
        if fd >= 0 {
            // SAFETY: a fresh fd nothing else owns.
            made = Some((unsafe { std::fs::File::from_raw_fd(fd) }, tmp));
            break;
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(format!("create a temp file in {}: {e}", dir.display()));
        }
    }
    let (mut f, tmp) = made.ok_or_else(|| format!("no free temp name in {}", dir.display()))?;
    let unlink = |why: String| {
        // SAFETY: a valid dir fd and C string; removes our own temp file.
        unsafe { libc::unlinkat(dfd.as_raw_fd(), tmp.as_ptr(), 0) };
        why
    };
    f.write_all(bytes).map_err(|e| unlink(format!("write {}: {e}", path.display())))?;
    f.sync_all().map_err(|e| unlink(format!("sync {}: {e}", path.display())))?;
    if let Some((uid, gid)) = owner {
        // SAFETY: plain getter.
        if unsafe { libc::geteuid() } == 0 && unsafe { libc::fchown(f.as_raw_fd(), uid, gid) } != 0 {
            return Err(unlink(format!("chown {}: {}", path.display(), std::io::Error::last_os_error())));
        }
    }
    // SAFETY: a valid fd.
    if unsafe { libc::fchmod(f.as_raw_fd(), mode as libc::mode_t) } != 0 {
        return Err(unlink(format!("chmod {}: {}", path.display(), std::io::Error::last_os_error())));
    }
    drop(f);
    let cname = std::ffi::CString::new(name.as_encoded_bytes()).map_err(|_| unlink(format!("{}: a NUL in the name", path.display())))?;
    // SAFETY: valid dir fd and C strings.
    if unsafe { libc::renameat(dfd.as_raw_fd(), tmp.as_ptr(), dfd.as_raw_fd(), cname.as_ptr()) } != 0 {
        return Err(unlink(format!("rename to {}: {}", path.display(), std::io::Error::last_os_error())));
    }
    Ok(())
}

/// An exclusive `flock` on a lock file, held until dropped. It excludes
/// other threads (each holder opens its own file description) and other
/// processes alike: root's executor and `sudo k2-node …` at the machine
/// edit the same root records. Never take the same lock twice on one
/// thread (it would wait on itself).
#[derive(Debug)]
pub struct FileLock {
    _f: std::fs::File,
}

impl FileLock {
    /// Wait for the lock on `path`. A missing lock file is created 0644
    /// (empty, never written); it's opened read-only and never through a
    /// symlink, so anyone who may change what it guards can take it (the
    /// machine owner's group in a shared config dir) and nothing else
    /// happens to the file. Its parent must exist.
    pub fn exclusive(path: &Path) -> Result<Self, String> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| format!("{}: a NUL in the path", path.display()))?;
        // SAFETY: a valid C string; the fd is owned by the File below.
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0o644 as libc::c_uint) };
        if fd < 0 {
            return Err(format!("open {}: {}", path.display(), std::io::Error::last_os_error()));
        }
        // SAFETY: a fresh fd nothing else owns.
        let f = unsafe { std::fs::File::from_raw_fd(fd) };
        if !f.metadata().map_err(|e| format!("stat {}: {e}", path.display()))?.is_file() {
            return Err(format!("{} isn't a plain file", path.display()));
        }
        loop {
            // SAFETY: a valid open fd.
            if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } == 0 {
                return Ok(Self { _f: f });
            }
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::Interrupted {
                return Err(format!("lock {}: {e}", path.display()));
            }
        }
    }
}

/// `mkdir -p` with a mode on the leaf.
pub fn mkdir_mode(path: &Path, mode: u32) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| format!("create {}: {e}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|e| format!("chmod {}: {e}", path.display()))
}

/// Output of a finished command.
#[derive(Debug)]
pub struct CmdOut {
    pub ok: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Run a program with an explicit env (cleared first) and a time limit.
/// Never inherits the node's environment.
pub async fn run_cmd(
    program: &str,
    args: &[&str],
    env: &[(String, String)],
    cwd: Option<&Path>,
    limit: Duration,
) -> Result<CmdOut, String> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args).env_clear().kill_on_drop(true);
    for (k, v) in env {
        cmd.env(k, v);
    }
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    cmd.stdin(std::process::Stdio::null());
    let fut = cmd.output();
    match tokio::time::timeout(limit, fut).await {
        Err(_) => Err(format!("{program} timed out after {}s", limit.as_secs())),
        Ok(Err(e)) => Err(format!("{program}: {e}")),
        Ok(Ok(out)) => Ok(CmdOut {
            ok: out.status.success(),
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }),
    }
}

/// Run a prepared command (blocking) with a time limit; stdout on
/// success. Kills it on timeout. For fixed, absolute-path probes.
pub fn output_limited(mut cmd: std::process::Command, limit: Duration) -> Result<String, String> {
    use std::io::Read;
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let mut out = child.stdout.take().ok_or("no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {}s", limit.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(reader.join().unwrap_or_default())
}

/// The base PATH for jobs and node-run tools.
pub fn base_path(home: &Path) -> String {
    k2_node_proto::env::clean_path(&format!(
        "{}:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        home.join("toolchains/cargo/bin").display()
    ))
}

/// When process `pid` started, in the kernel's own units (Linux: clock
/// ticks since boot from `/proc/<pid>/stat`; macOS: microseconds since the
/// epoch from `proc_pidinfo`). Only ever compared with itself: a pid and
/// its start time name one process, even after the pid is reused.
pub fn process_start_time(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // Field 2 (comm) may hold spaces and parentheses: split after the last ')'.
        let rest = &s[s.rfind(')')? + 1..];
        // `rest` starts at field 3; starttime is field 22.
        return rest.split_whitespace().nth(19)?.parse().ok();
    }
    #[cfg(target_os = "macos")]
    {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: a zeroed out-struct of the size we pass.
        let n = unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut libc::c_void, size) };
        if n != size {
            return None;
        }
        return Some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec);
    }
    #[allow(unreachable_code)]
    {
        let _ = pid;
        None
    }
}

/// A random-ish jitter in `0..max_ms` from the system CSPRNG.
pub fn jitter_ms(max_ms: u64) -> u64 {
    if max_ms == 0 {
        return 0;
    }
    let b = k2_node_proto::crypto::random_bytes(8).unwrap_or_else(|_| vec![0; 8]);
    let n = u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
    n % max_ms
}

/// Test helper: a fresh empty directory under the system temp dir.
pub fn temp_dir(tag: &str) -> std::path::PathBuf {
    let id = k2_node_proto::crypto::random_id().unwrap_or_else(|_| format!("{}", now()));
    let p = std::env::temp_dir().join(format!("k2-node-test-{tag}-{id}"));
    std::fs::create_dir_all(&p).expect("create temp dir");
    // macOS: /var → /private/var; git and cwd comparisons want the real path.
    std::fs::canonicalize(&p).expect("canonicalize temp dir")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A held lock keeps another holder (another thread here; another
    /// process alike) waiting until it's dropped; a lock file that is a
    /// symlink is refused.
    #[test]
    fn file_lock_excludes_until_dropped() {
        let d = temp_dir("filelock");
        let p = d.join("x.lock");
        let held = FileLock::exclusive(&p).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let p2 = p.clone();
        let t = std::thread::spawn(move || {
            let _g = FileLock::exclusive(&p2).unwrap();
            tx.send(()).unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "the second holder waits");
        drop(held);
        rx.recv_timeout(Duration::from_secs(5)).expect("the second holder gets it once the first drops it");
        t.join().unwrap();
        let link = d.join("link.lock");
        std::os::unix::fs::symlink(d.join("elsewhere"), &link).unwrap();
        assert!(FileLock::exclusive(&link).is_err(), "never through a symlink");
    }

    /// H1: a link planted at the target is replaced, never followed; a
    /// parent that is a link is refused; no temp file is left behind.
    #[test]
    fn atomic_write_never_follows_a_link() {
        let d = temp_dir("atomic");
        let victim = d.join("victim");
        std::fs::write(&victim, "keep me").unwrap();
        let target = d.join("out.json");
        std::os::unix::fs::symlink(&victim, &target).unwrap();
        atomic_write(&target, b"new", 0o644).unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me", "the link's target is untouched");
        assert!(!std::fs::symlink_metadata(&target).unwrap().file_type().is_symlink(), "the link itself was replaced");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o644);
        let real = d.join("real");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, d.join("linkdir")).unwrap();
        assert!(atomic_write(&d.join("linkdir/x"), b"x", 0o644).is_err(), "a parent that is a link is refused");
        assert!(!real.join("x").exists());
        let left: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.contains(".tmp-")).collect();
        assert!(left.is_empty(), "no temp files left: {left:?}");
    }

    /// H1 as root: root never writes into a directory another user owns.
    #[test]
    #[ignore = "root: run in the Linux container with --ignored"]
    fn root_never_writes_into_another_users_dir() {
        assert_eq!(unsafe { libc::geteuid() }, 0, "run as root");
        let d = temp_dir("atomic-root");
        let theirs = d.join("theirs");
        std::fs::create_dir(&theirs).unwrap();
        std::os::unix::fs::chown(&theirs, Some(65534), Some(65534)).unwrap();
        let e = atomic_write(&theirs.join("mark.json"), b"x", 0o644).unwrap_err();
        assert!(e.contains("root never writes into"), "{e}");
        atomic_write_owned(&d.join("mine.json"), b"x", 0o640, Some((0, 65534))).unwrap();
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(d.join("mine.json")).unwrap();
        assert_eq!((m.uid(), m.gid(), m.mode() & 0o777), (0, 65534, 0o640));
    }
}
