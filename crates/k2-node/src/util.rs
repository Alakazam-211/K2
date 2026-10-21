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
    let dir = path.parent().ok_or_else(|| format!("{} has no parent", path.display()))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("file"),
        std::process::id()
    ));
    {
        let mut f = std::fs::File::create(&tmp).map_err(|e| format!("create {}: {e}", tmp.display()))?;
        f.write_all(bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
        f.sync_all().map_err(|e| format!("sync {}: {e}", tmp.display()))?;
    }
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))
        .map_err(|e| format!("chmod {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename to {}: {e}", path.display()))?;
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
}
