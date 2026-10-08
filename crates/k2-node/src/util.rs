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
