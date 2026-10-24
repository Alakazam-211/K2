//! One `k2 job` run: passport, vault delivery, process, timeout, log.

use std::collections::HashMap;
use std::fs::{DirBuilder, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use k2_core::jobs::{self as store, Job, JobError};
use k2_core::log_debug;
use parking_lot::Mutex;

use super::{Live, STOP_KILL, STOP_NONE};

/// A run's log keeps at most this much output (the rest is counted).
pub const LOG_CAP_BYTES: u64 = 1024 * 1024;
/// SIGTERM → SIGKILL grace on timeout or kill.
const KILL_GRACE: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);

/// `~/.k2/jobs`.
pub fn jobs_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".k2").join("jobs")
}

fn private_dir(d: &Path) -> std::io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(d)
}

/// Start a run of `job` now. `trigger`: schedule | catchup | manual.
/// Returns the run id once the process is started; the run finishes on its
/// own thread. A scheduled slot that finds a run in flight (or the
/// daemon-wide cap reached) is recorded as a skipped row.
pub fn start_run(job: &Job, trigger: &str, started_by: &str) -> Result<String, JobError> {
    let db = k2_core::db::shared();
    let now = Utc::now();
    let run_id = super::new_run_id();
    let manual = trigger == "manual";
    if super::live_count() >= super::MAX_CONCURRENT_RUNS {
        if !manual {
            let conn = db.lock();
            let _ = store::insert_run(&conn, &run_id, &job.id, "skipped_busy", trigger, None, started_by, now);
        }
        return Err(JobError::new("too_many_runs", 409, format!("{} job runs are already going; try again soon", super::MAX_CONCURRENT_RUNS)));
    }
    let session_id = format!("{}{}", crate::session_token::JOB_SID_PREFIX, run_id);
    let live = super::claim(Live {
        job_id: job.id.clone(),
        run_id: run_id.clone(),
        workspace_id: job.workspace_id.clone(),
        name: job.name.clone(),
        session_id: session_id.clone(),
        stop: std::sync::atomic::AtomicU8::new(STOP_NONE),
    });
    let Some(live) = live else {
        if !manual {
            let conn = db.lock();
            let _ = store::insert_run(&conn, &run_id, &job.id, "skipped_overlap", trigger, None, started_by, now);
        }
        return Err(JobError::new("job_running", 409, format!("{} is already running (k2 job kill {} stops it)", job.name, job.name)));
    };

    let log_path = jobs_dir().join(&job.workspace_id).join(&job.name).join(format!("{run_id}.log"));
    {
        let conn = db.lock();
        if let Err(e) = store::insert_run(&conn, &run_id, &job.id, "running", trigger, Some(&log_path.display().to_string()), started_by, now) {
            super::release(&job.id, &run_id);
            return Err(e);
        }
    }
    match spawn(job, &live, &log_path) {
        Ok(started) => {
            let pid = started.child.id() as i64;
            {
                let conn = db.lock();
                let _ = store::set_run_pid(&conn, &run_id, pid);
            }
            let job = job.clone();
            let live2 = live.clone();
            let r = std::thread::Builder::new()
                .name(format!("k2-job-{}", job.name))
                .spawn(move || supervise(job, live2, started));
            if let Err(e) = r {
                // The run can't be watched: stop it now rather than leave it loose.
                unsafe { libc::killpg(pid as i32, libc::SIGKILL) };
                finish(&live, None, "spawn_failed", None, 0);
                return Err(JobError::new("spawn_failed", 500, format!("couldn't watch the run: {e}")));
            }
            Ok(run_id)
        }
        Err((e, mat)) => {
            if let Some(m) = mat {
                crate::vault::spawn::job_ended(&m);
            }
            let _ = std::fs::write(&log_path, format!("[k2] the job didn't start: {}\n", e.hint));
            finish(&live, None, "spawn_failed", None, 0);
            Err(e)
        }
    }
}

struct Started {
    child: std::process::Child,
    sink: Arc<Mutex<Sink>>,
    readers: Vec<std::thread::JoinHandle<()>>,
    materialized: crate::vault::spawn::Materialized,
}

/// The run's working folder: the workspace folder, or a folder inside it
/// (checked on disk at every run: no symlink out).
pub fn run_dir(workspace_root: &Path, rel: &str) -> Result<PathBuf, JobError> {
    let bad = || JobError::new("path_outside_workspace", 400, "the job's folder isn't inside its workspace");
    let root = std::fs::canonicalize(workspace_root)
        .map_err(|_| JobError::new("workspace_missing", 409, "the workspace folder isn't there"))?;
    let rel = store::validate_cwd(rel)?;
    if rel.is_empty() {
        return Ok(root);
    }
    let p = std::fs::canonicalize(root.join(&rel)).map_err(|_| JobError::new("cwd_missing", 409, format!("{rel} isn't a folder in the workspace")))?;
    if !p.starts_with(&root) {
        return Err(bad());
    }
    if !p.is_dir() {
        return Err(JobError::new("cwd_missing", 409, format!("{rel} isn't a folder in the workspace")));
    }
    Ok(p)
}

/// PATH for a run: the `k2` CLI's homes first (`/usr/local/bin` on
/// macOS and Linux, `~/.local/bin` the Linux fallback, Homebrew), then the
/// daemon's own PATH, then the system folders. Cron's `/usr/bin:/bin`
/// has no `k2`; a job always does.
pub fn job_path(daemon_path: Option<&str>, home: Option<&Path>) -> String {
    let mut out: Vec<String> = vec![];
    let mut add = |p: String| {
        if !p.is_empty() && !out.contains(&p) {
            out.push(p);
        }
    };
    add("/usr/local/bin".into());
    if let Some(h) = home {
        add(h.join(".local/bin").display().to_string());
    }
    if cfg!(target_os = "macos") {
        add("/opt/homebrew/bin".into());
    }
    for p in daemon_path.unwrap_or("").split(':') {
        add(p.to_string());
    }
    for p in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
        add(p.to_string());
    }
    out.join(":")
}

/// The run's environment: clean (never the daemon's own, which may hold
/// K2's tokens), plus the passport, the CLI's port and the vault items.
fn job_env(job: &Job, root: &Path, run_id: &str, passport: &str) -> HashMap<String, String> {
    let mut env = HashMap::new();
    for k in ["HOME", "USER", "LOGNAME", "SHELL", "LANG", "LC_ALL", "LC_CTYPE", "TZ", "TMPDIR", "XDG_RUNTIME_DIR"] {
        if let Ok(v) = std::env::var(k) {
            if !v.is_empty() {
                env.insert(k.to_string(), v);
            }
        }
    }
    let home = dirs::home_dir();
    env.insert("PATH".into(), job_path(std::env::var("PATH").ok().as_deref(), home.as_deref()));
    let root_s = root.display().to_string();
    for k in ["K2_PROJECT_PATH", "K2SO_PROJECT_PATH"] {
        env.insert(k.into(), root_s.clone());
    }
    let port = k2_core::hook_config::get_port();
    if port > 0 {
        env.insert("K2_PORT".into(), port.to_string());
        env.insert("K2SO_PORT".into(), port.to_string());
    }
    // The passport rides the key the CLI already reads. No K2_HOOK_SOCK: a
    // job has no cell socket, so the CLI uses loopback TCP with the
    // passport, never the owner token (cli/k2 _cli_apply_auth_for_verb).
    env.insert("K2_HOOK_TOKEN".into(), passport.to_string());
    env.insert("K2SO_HOOK_TOKEN".into(), passport.to_string());
    env.insert("K2_JOB".into(), job.name.clone());
    env.insert("K2_JOB_RUN".into(), run_id.to_string());
    env
}

#[allow(clippy::type_complexity)]
fn spawn(job: &Job, live: &Arc<Live>, log_path: &Path) -> Result<Started, (JobError, Option<crate::vault::spawn::Materialized>)> {
    let root = crate::vault::workspace_path(&job.workspace_id)
        .ok_or_else(|| (JobError::new("workspace_missing", 409, "the job's workspace is gone"), None))?;
    let cwd = run_dir(Path::new(&root), &job.cwd).map_err(|e| (e, None))?;
    let root = std::fs::canonicalize(&root).unwrap_or_else(|_| PathBuf::from(&root));
    if let Some(parent) = log_path.parent() {
        private_dir(parent).map_err(|e| (JobError::new("log_failed", 500, format!("log folder: {}", e.kind())), None))?;
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(log_path)
        .map_err(|e| (JobError::new("log_failed", 500, format!("log file: {}", e.kind())), None))?;
    let passport = crate::session_token::mint_job_passport(&live.session_id, &job.workspace_id, &job.name);
    let mut env = job_env(job, &root, &live.run_id, &passport);
    let mat = crate::vault::spawn::materialize_job(&mut env, &job.workspace_id, &job.id, &job.name, &live.session_id);
    let sink = Arc::new(Mutex::new(Sink { file, written: 0, total: 0, capped: false }));
    if !mat.skipped.is_empty() {
        sink.lock().note(&format!("[k2] vault items not delivered: {}\n", mat.skipped.join("; ")));
    }
    let mut cmd = Command::new(&job.argv[0]);
    cmd.args(&job.argv[1..])
        .current_dir(&cwd)
        .env_clear()
        .envs(&env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            crate::session_token::revoke_job_passport(&live.session_id);
            return Err((JobError::new("spawn_failed", 409, format!("couldn't start {}: {e}", job.argv[0])), Some(mat)));
        }
    };
    let redactor = crate::vault::redactor();
    let mut readers = vec![];
    for pipe in [child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>), child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>)]
        .into_iter()
        .flatten()
    {
        let sink = sink.clone();
        let redactor = redactor.clone();
        readers.push(std::thread::spawn(move || pump(pipe, &redactor, &sink)));
    }
    Ok(Started { child, sink, readers, materialized: mat })
}

/// The capped log. Bytes past the cap are counted, not kept.
struct Sink {
    file: std::fs::File,
    written: u64,
    total: u64,
    capped: bool,
}

impl Sink {
    fn push(&mut self, redacted: &[u8], raw_len: usize) {
        self.total += raw_len as u64;
        if self.capped || redacted.is_empty() {
            return;
        }
        let room = LOG_CAP_BYTES.saturating_sub(self.written) as usize;
        let take = redacted.len().min(room);
        if self.file.write_all(&redacted[..take]).is_ok() {
            self.written += take as u64;
        }
        if take < redacted.len() {
            self.capped = true;
        }
    }

    /// K2's own lines (not counted as job output, never capped away).
    fn note(&mut self, s: &str) {
        let _ = self.file.write_all(s.as_bytes());
    }
}

/// Copy one pipe into the log through the vault redactor (a value split
/// across reads is still caught: `StreamRedactor` holds back a tail).
fn pump(mut pipe: Box<dyn Read + Send>, redactor: &k2_vault::redact::Redactor, sink: &Mutex<Sink>) {
    let mut stream = redactor.stream();
    let mut buf = [0u8; 8192];
    loop {
        match pipe.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let out = stream.push(&buf[..n]);
                sink.lock().push(&out, n);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    let tail = stream.finish();
    sink.lock().push(&tail, 0);
}

/// Wait for the run, enforce the timeout and the kill switch, then clean up.
fn supervise(job: Job, live: Arc<Live>, mut s: Started) {
    let pgid = s.child.id() as i32;
    let deadline = Instant::now() + Duration::from_secs(job.timeout_secs);
    let mut term_at: Option<Instant> = None;
    let mut reason: Option<&'static str> = None;
    let status = loop {
        match s.child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {}
            Err(e) => {
                log_debug!("[jobs] wait {}: {e}", job.name);
                break None;
            }
        }
        let now = Instant::now();
        if term_at.is_none() {
            let why = if live.stop.load(Ordering::SeqCst) == STOP_KILL {
                Some("killed")
            } else if now >= deadline {
                Some("timeout")
            } else {
                None
            };
            if let Some(w) = why {
                reason = Some(w);
                term_at = Some(now);
                unsafe { libc::killpg(pgid, libc::SIGTERM) };
            }
        } else if term_at.is_some_and(|t| now.duration_since(t) >= KILL_GRACE) {
            unsafe { libc::killpg(pgid, libc::SIGKILL) };
        }
        std::thread::sleep(POLL);
    };
    // Anything the job left behind in its group goes with it (and lets the
    // pipes close so the log readers finish).
    unsafe { libc::killpg(pgid, libc::SIGKILL) };
    for r in s.readers.drain(..) {
        let _ = r.join();
    }
    let (total, capped) = {
        let mut sink = s.sink.lock();
        if sink.capped {
            let msg = format!("\n[k2] log capped at 1 MiB; {} more bytes not kept\n", sink.total.saturating_sub(sink.written));
            sink.note(&msg);
        }
        match reason {
            Some("timeout") => sink.note(&format!("\n[k2] stopped: timeout after {}\n", store::human_secs(job.timeout_secs))),
            Some("killed") => sink.note("\n[k2] stopped: k2 job kill\n"),
            _ => {}
        }
        let _ = sink.file.sync_all();
        (sink.total, sink.capped)
    };
    let _ = capped;
    let code = status.and_then(|st| st.code()).map(|c| c as i64);
    let signal = status.and_then(|st| st.signal());
    let outcome = match (reason, code) {
        (Some(r), _) => r,
        (None, Some(0)) => "ok",
        (None, _) => {
            if let Some(sig) = signal {
                log_debug!("[jobs] {} ended by signal {sig}", job.name);
            }
            "failed"
        }
    };
    crate::vault::spawn::job_ended(&s.materialized);
    finish(&live, code, outcome, None, total);
}

/// Revoke the passport, free the slot, close the row, prune old logs.
fn finish(live: &Live, code: Option<i64>, outcome: &str, _note: Option<&str>, bytes: u64) {
    crate::session_token::revoke_job_passport(&live.session_id);
    super::release(&live.job_id, &live.run_id);
    let db = k2_core::db::shared();
    let drop_logs = {
        let conn = db.lock();
        if let Err(e) = store::finish_run(&conn, &live.run_id, outcome, code, bytes, Utc::now()) {
            log_debug!("[jobs] finish {}: {e}", live.name);
        }
        store::prune_runs(&conn, &live.job_id).unwrap_or_default()
    };
    for p in drop_logs {
        let _ = std::fs::remove_file(p);
    }
}

/// Boot: stop a group a previous daemon's run left behind, only when its
/// leader is provably that run (Linux: its environ carries
/// `K2_JOB_RUN=<run id>`). Elsewhere the group is left alone: its
/// passport is already revoked, so it can't use K2.
pub fn stop_leftover_group(pid: i64, run_id: &str) {
    #[cfg(target_os = "linux")]
    {
        let Ok(environ) = std::fs::read(format!("/proc/{pid}/environ")) else { return };
        let want = format!("K2_JOB_RUN={run_id}");
        if environ.split(|b| *b == 0).any(|kv| kv == want.as_bytes()) {
            unsafe { libc::killpg(pid as i32, libc::SIGKILL) };
            log_debug!("[jobs] boot: stopped run {run_id}'s leftover processes");
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (pid, run_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_puts_the_cli_homes_first_and_dedupes() {
        let p = job_path(Some("/usr/bin:/custom/bin:/usr/local/bin"), Some(Path::new("/home/k2")));
        let parts: Vec<&str> = p.split(':').collect();
        assert_eq!(parts[0], "/usr/local/bin");
        assert_eq!(parts[1], "/home/k2/.local/bin");
        assert!(parts.contains(&"/custom/bin"));
        assert_eq!(parts.iter().filter(|x| **x == "/usr/local/bin").count(), 1);
        assert!(parts.contains(&"/bin") && parts.contains(&"/usr/sbin"));
        // Cron's PATH alone still gets the CLI's home.
        assert!(job_path(Some("/usr/bin:/bin"), None).starts_with("/usr/local/bin:"));
    }

    #[test]
    fn run_dir_stays_inside_the_workspace() {
        let t = k2_vault::fsutil::TestDir::new("job-cwd");
        let root = t.path().join("ws");
        std::fs::create_dir_all(root.join("scripts")).expect("mkdir");
        std::fs::create_dir_all(t.path().join("outside")).expect("mkdir");
        std::os::unix::fs::symlink(t.path().join("outside"), root.join("escape")).expect("symlink");
        let canon = std::fs::canonicalize(&root).expect("canon");
        assert_eq!(run_dir(&root, "").expect("root"), canon);
        assert_eq!(run_dir(&root, "scripts").expect("sub"), canon.join("scripts"));
        assert_eq!(run_dir(&root, "escape").expect_err("symlink out").code, "path_outside_workspace");
        assert_eq!(run_dir(&root, "../outside").expect_err("dotdot").code, "path_outside_workspace");
        assert_eq!(run_dir(&root, "nope").expect_err("missing").code, "cwd_missing");
        assert_eq!(run_dir(&t.path().join("gone"), "").expect_err("no ws").code, "workspace_missing");
    }

    #[test]
    fn the_sink_caps_and_counts() {
        let t = k2_vault::fsutil::TestDir::new("job-sink");
        let file = std::fs::File::create(t.path().join("x.log")).expect("file");
        let mut s = Sink { file, written: 0, total: 0, capped: false };
        let chunk = vec![b'a'; 700 * 1024];
        s.push(&chunk, chunk.len());
        s.push(&chunk, chunk.len());
        s.push(&chunk, chunk.len());
        assert!(s.capped);
        assert_eq!(s.written, LOG_CAP_BYTES);
        assert_eq!(s.total, 3 * 700 * 1024);
        assert_eq!(std::fs::metadata(t.path().join("x.log")).expect("m").len(), LOG_CAP_BYTES);
    }

    #[test]
    fn pump_redacts_a_value_split_across_reads() {
        let secret = "k2test_PUMP_9f8e7d6c5b4a3210";
        let r = k2_vault::redact::Redactor::new([("PUMP_KEY", secret.as_bytes())]);
        let t = k2_vault::fsutil::TestDir::new("job-pump");
        let path = t.path().join("p.log");
        let file = std::fs::File::create(&path).expect("file");
        let sink = Mutex::new(Sink { file, written: 0, total: 0, capped: false });
        // A reader that hands the value over in two reads.
        struct Halves(Vec<Vec<u8>>);
        impl Read for Halves {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.0.is_empty() {
                    return Ok(0);
                }
                let c = self.0.remove(0);
                buf[..c.len()].copy_from_slice(&c);
                Ok(c.len())
            }
        }
        let line = format!("token={secret}\n");
        let (a, b) = line.as_bytes().split_at(14);
        pump(Box::new(Halves(vec![a.to_vec(), b.to_vec()])), &r, &sink);
        let got = std::fs::read_to_string(&path).expect("log");
        assert!(!got.contains(secret), "{got}");
        assert!(got.contains("[k2v:PUMP_KEY]"), "{got}");
        assert_eq!(sink.lock().total, line.len() as u64);
    }
}
