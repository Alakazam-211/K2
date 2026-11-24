//! Daemon-owned published-service supervisor (prd-k2-publish-hosted-services-v1).
//!
//! Spawns a subprocess + process group (Unix `setsid`) / Job Object
//! (Windows, required). Never registered in `v2_session_map`. Never
//! added to the frpc `pkill -f` pattern. Child exit → status `exited`,
//! `desired` stays `running` unless someone `stop`s (no crash loop).

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use k2_core::db::schema::SubdomainWorkspace;
use k2_core::frame_policy::{self, FramePolicy};
use k2_core::{cli_name, log_debug, product_name};

use k2_core::published_services::{
    self as ps, ServiceJson, CMD_SKIN_SENTINEL, DESIRED_RUNNING, DESIRED_STOPPED, EXPOSE_LOCAL,
    EXPOSE_TUNNEL, KIND_CMD, KIND_SKIN, STATUS_EXITED, STATUS_IDLE_STOPPED, STATUS_RUNNING,
    STATUS_STARTING, STATUS_STOPPED, STATUS_UNHEALTHY,
};
use crate::site_gate::dev::DevWake;
use k2_core::site_visibility as sv;
use k2_core::terminal::login_path;
use k2_core::tunnel::config as tunnel_config;
use k2_core::tunnel::subdomains;
use k2_core::brand::{CLI_NAME, PRODUCT_NAME};

const LOG_CAP: u64 = 2 * 1024 * 1024;
#[cfg(unix)]
const KILL_GRACE: Duration = Duration::from_millis(400);
/// Windows: how long a stop waits for the service's job to empty after
/// TerminateJobObject before it reports the stop failed.
#[cfg(windows)]
const STOP_WAIT_CAP: Duration = Duration::from_secs(10);
const DEFAULT_PROBE: Duration = Duration::from_millis(ps::PROBE_TIMEOUT_DEFAULT_MS);

/// Current k2-daemon loopback port. Used to synthesize skin-gateway `--upstream`.
/// Never persisted in `published_services.cmd`.
static LOOPBACK_PORT: AtomicU16 = AtomicU16::new(0);

pub fn set_loopback_port(port: u16) {
    LOOPBACK_PORT.store(port, Ordering::SeqCst);
}

pub fn loopback_port() -> u16 {
    let p = LOOPBACK_PORT.load(Ordering::SeqCst);
    if p != 0 {
        return p;
    }
    let path = k2_core::paths::k2_home().join("daemon.port");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

#[derive(Debug)]
pub struct PublishError {
    pub status: &'static str,
    pub message: String,
    /// Structured JSON body (e.g. a label-ownership refusal); when set it
    /// is sent as-is instead of `{"error": message}`.
    pub body: Option<serde_json::Value>,
}

impl PublishError {
    /// A refusal with its own code (`{error, code}`, the CLI's exit 3).
    pub(crate) fn coded(code: &str, msg: impl Into<String>) -> Self {
        let message = msg.into();
        Self {
            status: "400 Bad Request",
            body: Some(serde_json::json!({ "error": message, "code": code })),
            message,
        }
    }
    fn bad(msg: impl Into<String>) -> Self {
        Self {
            status: "400 Bad Request",
            message: msg.into(),
            body: None,
        }
    }
    fn target(r: crate::splice_guard::TargetRefusal) -> Self {
        Self {
            status: "400 Bad Request",
            message: r.to_string(),
            body: Some(serde_json::json!({ "error": r.to_string(), "code": r.code })),
        }
    }
    fn fail(msg: impl Into<String>) -> Self {
        Self {
            status: "400 Bad Request",
            message: msg.into(),
            body: None,
        }
    }
}

impl From<crate::label_owner::Refusal> for PublishError {
    fn from(r: crate::label_owner::Refusal) -> Self {
        Self {
            status: r.status,
            message: r.message(),
            body: Some(r.body),
        }
    }
}

struct LiveChild {
    project_id: String,
    name: String,
    pid: i32,
    #[allow(dead_code)]
    port: u16,
    started: Instant,
    /// Startup grace for this child (`--probe-timeout`, else the default):
    /// `starting` until it runs out, then `unhealthy` (local) or rolled
    /// back (tunnel).
    grace: Duration,
    probe_ok: AtomicBool,
    stop_requested: AtomicBool,
    child: Mutex<Option<Child>>,
    #[cfg(windows)]
    job: Mutex<Option<win_job::Job>>,
}

type LiveMap = HashMap<(String, String), Arc<LiveChild>>;

fn live() -> &'static Mutex<LiveMap> {
    static LIVE: OnceLock<Mutex<LiveMap>> = OnceLock::new();
    LIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn live_get(project_id: &str, name: &str) -> Option<Arc<LiveChild>> {
    live()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(project_id.to_string(), name.to_string()))
        .cloned()
}

fn live_insert(entry: Arc<LiveChild>) {
    let key = (entry.project_id.clone(), entry.name.clone());
    live()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, entry);
}

/// Every live `--cmd`/skin child: (workspace id, publish name, pid). Each
/// child leads its own session (`setsid`), so its pid is also the session
/// id of everything it starts (0.46.0 owner-token attribution).
pub fn live_children() -> Vec<(String, String, i32)> {
    live()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .map(|c| (c.project_id.clone(), c.name.clone(), c.pid))
        .collect()
}

fn live_remove(project_id: &str, name: &str) -> Option<Arc<LiveChild>> {
    live()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&(project_id.to_string(), name.to_string()))
}

/// The default startup grace, and the longest a start request waits in
/// line for the port: 15 s, or `K2_PUBLISH_PROBE_MS` (tests).
fn probe_timeout() -> Duration {
    std::env::var("K2_PUBLISH_PROBE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(|ms| Duration::from_millis(ms.max(50)))
        .unwrap_or(DEFAULT_PROBE)
}

/// This service's startup grace: its stored `--probe-timeout` (capped at
/// 15 min), else [`probe_timeout`].
fn grace_for(row: &ps::PublishedService) -> Duration {
    match row.probe_timeout_ms {
        Some(ms) if ms > 0 => Duration::from_millis((ms as u64).min(ps::PROBE_TIMEOUT_MAX_MS)),
        _ => probe_timeout(),
    }
}

/// How often a service inside its grace is re-probed (1 s;
/// `K2_PUBLISH_REPROBE_MS` for tests).
fn reprobe_interval() -> Duration {
    std::env::var("K2_PUBLISH_REPROBE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(|ms| Duration::from_millis(ms.max(20)))
        .unwrap_or(Duration::from_secs(1))
}

/// How often a local service past its grace (status `unhealthy`) is
/// re-probed so a late bind still turns it `running` (5 s;
/// `K2_PUBLISH_SUPERVISE_MS` for tests).
fn supervise_interval() -> Duration {
    std::env::var("K2_PUBLISH_SUPERVISE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(|ms| Duration::from_millis(ms.max(20)))
        .unwrap_or(Duration::from_secs(5))
}

/// `800ms`, `2.5s`, `90s` → `1m30s`, `10m` — for errors and status lines.
pub fn fmt_duration(d: Duration) -> String {
    let ms = d.as_millis() as u64;
    if ms < 1000 {
        return format!("{ms}ms");
    }
    if ms % 1000 != 0 && ms < 10_000 {
        return format!("{:.1}s", ms as f64 / 1000.0);
    }
    let secs = ms / 1000;
    match (secs / 60, secs % 60) {
        (0, s) => format!("{s}s"),
        (m, 0) => format!("{m}m"),
        (m, s) => format!("{m}m{s}s"),
    }
}

/// The rollback error when a service never opened its port in its grace.
fn grace_miss_message(port: u16, grace: Duration) -> String {
    format!(
        "probe failed: process started but 127.0.0.1:{port} never accepted within {}; \
         raise --probe-timeout ({CLI_NAME} publish run <name> ... --probe-timeout 10m, at most 15m)",
        fmt_duration(grace)
    )
}

fn claim_tries() -> u32 {
    std::env::var("K2_PUBLISH_CLAIM_TRIES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20)
        .max(1)
}

fn stagger() -> Duration {
    std::env::var("K2_PUBLISH_STAGGER_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(|ms| Duration::from_millis(ms))
        .unwrap_or(Duration::from_millis(200))
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn log_path(project_id: &str, name: &str) -> PathBuf {
    k2_core::paths::k2_home()
        .join("publish-logs")
        .join(project_id)
        .join(format!("{name}.log"))
}

fn rotate_if_needed(path: &Path) {
    let Ok(meta) = fs::metadata(path) else { return };
    if meta.len() <= LOG_CAP {
        return;
    }
    let bak = path.with_extension("log.1");
    let _ = fs::remove_file(&bak);
    let _ = fs::rename(path, &bak);
}

fn open_log(project_id: &str, name: &str) -> Result<std::fs::File, String> {
    let path = log_path(project_id, name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    rotate_if_needed(&path);
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("open {}: {e}", path.display()))
}

/// Rotate a log a LIVE service writes into directly (runtime stage H):
/// copy it to `<name>.log.1`, then truncate it in place. The service keeps
/// its `O_APPEND` descriptor, so its next write lands at the new end — a
/// rename (as [`rotate_if_needed`] does before a spawn) would leave it
/// writing into the backup forever. Lines written between the copy and
/// the truncate are lost; that is the price of not owning a pipe.
fn rotate_in_place_if_needed(path: &Path) {
    let Ok(meta) = fs::metadata(path) else { return };
    if meta.len() <= LOG_CAP {
        return;
    }
    let bak = path.with_extension("log.1");
    if fs::copy(path, &bak).is_ok() {
        if let Ok(f) = OpenOptions::new().write(true).open(path) {
            let _ = f.set_len(0);
        }
    }
}

/// The service's stdout and stderr: the log file itself, opened
/// `O_APPEND`. Before stage H these were pipes pumped by daemon threads,
/// so a daemon exit (restart, hand-off) closed them and the service's next
/// write hit `EPIPE`/`SIGPIPE` — published services and skin gateway
/// helpers died with the daemon even when nothing killed them.
fn log_stdio(path: &Path) -> Result<(Stdio, Stdio), String> {
    let f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    let f2 = f
        .try_clone()
        .map_err(|e| format!("clone log handle: {e}"))?;
    Ok((Stdio::from(f), Stdio::from(f2)))
}

pub fn read_log_tail(project_id: &str, name: &str, lines: usize) -> String {
    let path = log_path(project_id, name);
    let Ok(data) = fs::read_to_string(&path) else {
        return String::new();
    };
    let all: Vec<&str> = data.lines().collect();
    let start = all.len().saturating_sub(lines.max(1));
    all[start..].join("\n")
}

fn port_accepts(port: u16) -> bool {
    let addr = format!("127.0.0.1:{port}");
    let Ok(mut addrs) = addr.to_socket_addrs() else {
        return false;
    };
    let Some(sa) = addrs.next() else {
        return false;
    };
    TcpStream::connect_timeout(&sa, Duration::from_millis(200)).is_ok()
}

fn wait_for_port(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if port_accepts(port) {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    port_accepts(port)
}

fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(pid, 0) == 0
    }
    #[cfg(windows)]
    {
        win_job::pid_alive(pid as u32)
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

/// [`kill_tree_checked`] for callers that only log: a Windows job still
/// running at the cap is logged there and here.
fn kill_tree(entry: &LiveChild) {
    if let Err(e) = kill_tree_checked(entry) {
        log_debug!("[publish] stop: {e}");
    }
}

/// End the service and everything it started. `Err` (Windows only) when a
/// process of its job is still running after [`STOP_WAIT_CAP`]: the caller
/// must not report it stopped.
fn kill_tree_checked(entry: &LiveChild) -> Result<(), String> {
    entry.stop_requested.store(true, Ordering::SeqCst);
    #[cfg(unix)]
    {
        let pid = entry.pid;
        if pid <= 0 {
            return Ok(());
        }
        // C4: a child that left the process group stays in the agent's
        // root-owned scope; the helper stops the scope after the kill.
        let scope = k2_core::agent_scope::scope_of_pid(pid as u32);
        let _stop_scope = ScopeStop(scope);
        unsafe {
            let pgid = libc::getpgid(pid);
            if pgid == pid {
                let _ = libc::killpg(pgid, libc::SIGTERM);
                thread::sleep(KILL_GRACE);
                let pgid2 = libc::getpgid(pid);
                if pgid2 == pid {
                    let _ = libc::killpg(pgid2, libc::SIGKILL);
                } else if pgid2 < 0 {
                    let _ = libc::kill(pid, libc::SIGKILL);
                } else {
                    // Foreign group — never killpg it.
                    let _ = libc::kill(pid, libc::SIGKILL);
                }
            } else {
                let _ = libc::kill(pid, libc::SIGTERM);
                thread::sleep(KILL_GRACE);
                let _ = libc::kill(pid, libc::SIGKILL);
            }
        }
    }
    #[cfg(windows)]
    {
        if let Some(job) = entry.job.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            // TerminateJobObject only starts the end: wait until every
            // process of the job has exited (its sockets closed), like
            // killpg + wait on Unix, so an immediate start finds the port
            // free and a label return sees the service down.
            if let Err(left) = job.terminate_and_wait(STOP_WAIT_CAP) {
                let lines = k2_core::win_proc::command_lines(&left);
                let named: Vec<String> = left
                    .iter()
                    .map(|p| match lines.iter().find(|(q, _)| q == p) {
                        Some((_, c)) => format!("{p} ({c})"),
                        None => p.to_string(),
                    })
                    .collect();
                let msg = format!(
                    "still running {}s after the stop: {}",
                    STOP_WAIT_CAP.as_secs(),
                    named.join(", ")
                );
                log_debug!("[publish] stop pid {}: {msg}", entry.pid);
                return Err(msg);
            }
        } else {
            let killed_child = entry
                .child
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_mut()
                .map(|c| c.kill())
                .is_some();
            // A service adopted at boot (reattach) has neither a job nor a
            // child handle: end it and its children by pid, or it keeps
            // its port and a restart fails ("still in use").
            if !killed_child {
                if let Ok(pid) = u32::try_from(entry.pid) {
                    k2_core::win_proc::terminate_tree(pid);
                }
            }
        }
    }
    if let Some(mut child) = entry
        .child
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    {
        let _ = child.wait();
    }
    Ok(())
}

fn enrich_path(cmd: &mut Command) {
    let inherited = std::env::var("PATH")
        .or_else(|_| std::env::var("Path"))
        .unwrap_or_default();
    let path = login_path::augmented_path(&inherited);
    cmd.env("PATH", &path);
    #[cfg(windows)]
    {
        cmd.env("Path", &path);
    }
}

fn spawn_prepared(
    mut c: Command,
    project_id: &str,
    name: &str,
    enrich: bool,
) -> Result<(Child, i32, Option<WinJobSlot>), String> {
    // A daemon that handed off starts no service: the successor owns them.
    k2_core::handoff_state::guard()?;
    let _ = open_log(project_id, name)?;
    let log_p = log_path(project_id, name);
    let (out, err) = log_stdio(&log_p)?;
    c.stdin(Stdio::null()).stdout(out).stderr(err);
    // Clean mode (0.46.0 grace) sets the system PATH itself: no login-shell
    // enrichment, so a program found only through the daemon user's own
    // bin folders shows up now, not when it moves to an App.
    if enrich {
        enrich_path(&mut c);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            c.pre_exec(|| {
                k2_core::bg_proc::own_session()?;
                Ok(())
            });
        }
        let child = c.spawn().map_err(|e| format!("spawn: {e}"))?;
        let pid = child.id() as i32;
        Ok((child, pid, None))
    }
    #[cfg(windows)]
    {
        let (child, job) = win_job::spawn_in_job(&mut c)?;
        let pid = child.id() as i32;
        Ok((child, pid, Some(job)))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (c, project_id, name, log_p);
        Err("published services require Unix or Windows".into())
    }
}

/// Windows: run `line` as cmd.exe runs a typed command line. `arg()` would
/// escape inner quotes as `\"`, which cmd.exe doesn't read, mangling a
/// line like `python -c "…"`. cmd's own form is `/d /s /c "<line>"`:
/// `/s` strips just the outer quotes, `/d` skips AutoRun.
#[cfg(windows)]
pub(crate) fn cmd_exe_line(c: &mut Command, line: &str) {
    use std::os::windows::process::CommandExt;
    c.args(["/d", "/s", "/c"]).raw_arg(format!("\"{line}\""));
}

/// The command that runs a `--cmd` service's shell line: `sh -c <cmd>`, or for
/// a dev server on a Linux box with user scopes, the same line inside a capped
/// scope (DM-5). Without scopes it still runs; `ps` says "no caps".
#[cfg(unix)]
fn shell_command(cmd: &str, name: &str, dev: Option<(i64, f64)>) -> (Command, bool) {
    #[cfg(target_os = "linux")]
    if let Some((mem_mb, cpus)) = dev {
        match crate::publish_dev::user_scopes() {
            Ok(exe) => {
                let mut c = k2_core::bg_proc::command(exe);
                c.args(crate::publish_dev::scope_args(&crate::publish_dev::scope_unit(name), mem_mb, cpus, cmd));
                return (c, true);
            }
            Err(why) => log_debug!("[publish] {name}: dev server runs without caps: {why}"),
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (name, dev);
    let mut c = k2_core::bg_proc::command("sh");
    c.arg("-c").arg(cmd);
    (c, false)
}

/// The dev server's own environment (DM-1), set last so nothing a service's
/// env file names can move its bind address off loopback.
fn apply_dev_env(c: &mut Command, name: &str, port: u16, dev: bool) {
    if !dev {
        return;
    }
    let host = public_url(name, EXPOSE_TUNNEL).map(|u| u.trim_start_matches("https://").to_string());
    c.envs(crate::publish_dev::env_pairs(port, host.as_deref()));
}

fn spawn_child(
    cmd: &str,
    cwd: &Path,
    project_id: &str,
    name: &str,
    service: Option<(&str, u16)>,
    dev: Option<(i64, f64)>,
) -> Result<(Child, i32, Option<WinJobSlot>), String> {
    #[cfg(unix)]
    {
        // Agent Isolation (review F1): a workspace's `--cmd` service starts
        // through the exec helper, in its agent's cgroup and under its fence.
        // A dev server's capped user scope would put it outside that cgroup:
        // with the helper's scopes (B4) its caps are the scope's own
        // properties; otherwise it runs uncapped while isolation applies.
        let mut iso = crate::agent_isolation::for_service(project_id, cwd);
        if let (Some(i), Some((mem, cpus))) = (iso.as_mut(), dev) {
            i.scope = i.scope.take().map(|p| p.with_caps(mem, cpus)).transpose()?;
        }
        let mut spec_env: Option<String> = None;
        let (mut c, scoped) = match &iso {
            Some(iso) => {
                if dev.is_some() && iso.scope.is_none() {
                    log_debug!("[publish] {name}: dev server caps not applied: the agent's cgroup holds it");
                }
                let (helper, argv, spec) = k2_core::agent_cgroup::wrap(iso, "sh".into(), vec!["-c".into(), cmd.to_string()], &Default::default())
                    .map_err(|e| format!("agent isolation: {e}"))?;
                let mut c = k2_core::bg_proc::command(helper);
                c.args(argv);
                spec_env = Some(spec);
                (c, false)
            }
            None => shell_command(cmd, name, dev),
        };
        c.current_dir(cwd);
        // K2 vault (§10.5, Q16): clean env + granted secrets for new
        // services; services from before the vault keep inheriting.
        let mut enrich = true;
        if let Some((service_id, port)) = service {
            let applied = crate::vault::publish_env::apply_service(&mut c, service_id, name, project_id, cwd, port);
            enrich = !applied.fixed_path;
            // 0.46.18: listen on this server only, unless the owner said public.
            crate::publish_expose::apply(&mut c, service_id, port);
        }
        apply_dev_env(&mut c, name, service.map_or(0, |(_, port)| port), dev.is_some());
        if scoped {
            c.envs(crate::publish_dev::user_manager_env());
        }
        // Last: the vault's clean mode clears the environment above.
        if let Some(spec) = spec_env {
            c.env(k2_core::agent_fence::ENV_SPEC, spec);
        }
        let (mut child, pgid, slot) = spawn_prepared(c, project_id, name, enrich)?;
        // M1b: the root helper moves the waiting exec helper into the
        // agent's root-owned cgroup, or the service stops here.
        if let Some(iso) = iso.as_ref() {
            if let Err(e) = k2_core::agent_cgroup::place_spawned(iso, child.id(), None) {
                let _ = child.wait();
                return Err(e.to_string());
            }
        }
        Ok((child, pgid, slot))
    }
    #[cfg(windows)]
    {
        let mut c = k2_core::bg_proc::command("cmd.exe");
        c.current_dir(cwd);
        cmd_exe_line(&mut c, cmd);
        // The same vault rules as Unix: clean env + granted secrets for
        // new services; older ones keep inheriting.
        let mut enrich = true;
        if let Some((service_id, port)) = service {
            let applied = crate::vault::publish_env::apply_service(&mut c, service_id, name, project_id, cwd, port);
            enrich = !applied.fixed_path;
            // 0.46.18: listen on this server only, unless the owner said public.
            crate::publish_expose::apply(&mut c, service_id, port);
        }
        apply_dev_env(&mut c, name, service.map_or(0, |(_, port)| port), dev.is_some());
        spawn_prepared(c, project_id, name, enrich)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (cmd, cwd, project_id, name, service, dev);
        Err("published services require Unix or Windows".into())
    }
}

fn spawn_argv(
    exe: &Path,
    args: &[String],
    cwd: &Path,
    project_id: &str,
    name: &str,
) -> Result<(Child, i32, Option<WinJobSlot>), String> {
    let mut c = k2_core::bg_proc::command(exe);
    c.args(args).current_dir(cwd);
    spawn_prepared(c, project_id, name, true)
}

/// Resolve the k2-daemon binary for `--skin-gateway`. Prefer the live
/// daemon's current_exe; tests fall back to CARGO_BIN_EXE / sibling.
pub fn resolve_skin_gateway_exe() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("K2_SKIN_GATEWAY_EXE") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Ok(pb);
        }
    }
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_k2-daemon") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Ok(pb);
        }
    }
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let name = exe
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if name == "k2-daemon" || name == "k2-daemon.exe" {
        return Ok(exe);
    }
    if let Some(dir) = exe.parent() {
        for cand in ["k2-daemon", "k2-daemon.exe"] {
            let p = dir.join(cand);
            if p.is_file() {
                return Ok(p);
            }
            if let Some(parent) = dir.parent() {
                let p = parent.join(cand);
                if p.is_file() {
                    return Ok(p);
                }
            }
        }
    }
    Err("could not resolve k2-daemon binary for --skin-gateway".into())
}

/// Workspace-relative UI dir. Empty = bundled. Reject abs / `..` / escape.
pub fn validate_skin_root(workspace: &Path, raw: &str) -> Result<String, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Ok(String::new());
    }
    let p = Path::new(t);
    // `has_root` too: on Windows `/tmp` or `\tmp` is rooted on the current
    // drive without being absolute.
    if p.is_absolute() || p.has_root() {
        return Err("skinRoot must be a workspace-relative directory".into());
    }
    if p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err("skinRoot must not contain ..".into());
    }
    let ws = dunce::canonicalize(&workspace)
        .map_err(|e| format!("workspace path: {e}"))?;
    let joined = ws.join(p);
    if !joined.is_dir() {
        return Err(format!(
            "skinRoot is not a directory: {}",
            p.display()
        ));
    }
    let canon = dunce::canonicalize(&joined)
        .map_err(|e| format!("skinRoot: {e}"))?;
    if !canon.starts_with(&ws) {
        return Err("skinRoot escapes the workspace".into());
    }
    Ok(t.replace('\\', "/"))
}

fn spawn_skin_helper(
    row: &ps::PublishedService,
    project_id: &str,
    name: &str,
) -> Result<(Child, i32, Option<WinJobSlot>), String> {
    let daemon_port = loopback_port();
    if daemon_port == 0 {
        return Err("skin gateway: daemon loopback port is unknown".into());
    }
    let exe = resolve_skin_gateway_exe()?;
    let cwd = PathBuf::from(&row.cwd);
    let root = if row.skin_root.trim().is_empty() {
        None
    } else {
        let joined = cwd.join(row.skin_root.trim());
        let canon = dunce::canonicalize(&joined)
            .map_err(|e| format!("skinRoot: {e}"))?;
        Some(canon)
    };
    // A stored value this daemon would never write: refuse to start rather
    // than serve the App unguarded.
    let frame = FramePolicy::from_stored(&row.frame_ancestors)
        .map_err(|e| format!("frame policy: {e} ({CLI_NAME} publish frame {name} --reset)"))?;
    let args = crate::skin_gateway::helper_argv(
        row.port as u16,
        daemon_port,
        root.as_deref(),
        &frame,
        &row.id,
    );
    spawn_argv(&exe, &args, &cwd, project_id, name)
}

#[cfg(not(windows))]
type WinJobSlot = ();
#[cfg(windows)]
type WinJobSlot = win_job::Job;

fn emit_changed(project_id: &str) {
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::PublishServicesChanged {
        project_id: project_id.to_string(),
    });
}

pub(crate) fn primary_host() -> String {
    tunnel_config::load()
        .ok()
        .map(|c| c.subdomain.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let p = subdomains::current().primary.trim().to_ascii_lowercase();
            if p.is_empty() {
                None
            } else {
                Some(p)
            }
        })
        .unwrap_or_default()
}

fn public_url(name: &str, expose: &str) -> Option<String> {
    if expose != EXPOSE_TUNNEL {
        return None;
    }
    let primary = primary_host();
    if primary.is_empty() {
        return None;
    }
    Some(format!("https://{name}.{primary}.k2.dev"))
}

fn compute_status(row: &ps::PublishedService, live: Option<&LiveChild>) -> (&'static str, Option<i64>) {
    if let Some(l) = live {
        let pid = Some(l.pid as i64);
        if l.probe_ok.load(Ordering::SeqCst) {
            return (STATUS_RUNNING, pid);
        }
        if l.started.elapsed() < l.grace {
            return (STATUS_STARTING, pid);
        }
        return (STATUS_UNHEALTHY, pid);
    }
    if row.desired == DESIRED_STOPPED {
        return (STATUS_STOPPED, None);
    }
    if row.pid.is_some() && row.pid.and_then(|p| i32::try_from(p).ok()).map(pid_alive).unwrap_or(false)
    {
        if port_accepts(row.port as u16) {
            return (STATUS_RUNNING, row.pid);
        }
        return (STATUS_UNHEALTHY, row.pid);
    }
    if row.last_exit_code.is_some() || row.last_exited_at.is_some() {
        return (STATUS_EXITED, None);
    }
    (STATUS_EXITED, None)
}

pub fn to_json(row: &ps::PublishedService) -> ServiceJson {
    let live = live_get(&row.project_id, &row.name);
    let (status, pid) = if row.cmd == ps::CMD_APP_SENTINEL {
        // An App runtime row (AR5): the App supervisor owns it.
        (crate::apps::supervisor::service_status(&row.id).unwrap_or(STATUS_STOPPED), None)
    } else {
        compute_status(row, live.as_deref())
    };
    // DM-6: an idle stop is its own state, never a crash.
    let status = if status == STATUS_STOPPED && row.dev_mem_mb.is_some() && row.dev_idle_stopped_at.is_some() {
        STATUS_IDLE_STOPPED
    } else {
        status
    };
    let url = if row.hostname_pending {
        // Inside the startup grace: the nested hostname is not published yet.
        None
    } else if status == STATUS_STOPPED && row.error.is_some() && row.desired == DESIRED_STOPPED {
        // Hostname-fail path: do not advertise a public URL.
        // A probe failure rolls back before the hostname step: there is
        // no public URL to show either.
        if row.error.as_deref().is_some_and(|e| {
            e.contains("hostname")
                || e.starts_with("probe failed")
                || e.contains("pro_required")
                || e.contains("claim")
                || e.contains("nested")
                || e.contains("subdomain")
        }) {
            None
        } else {
            public_url(&row.name, &row.expose)
        }
    } else {
        public_url(&row.name, &row.expose)
    };
    let mut json = ServiceJson::from_row(row, status, url, pid);
    if row.cmd != ps::CMD_APP_SENTINEL {
        let grace = live.as_deref().map(|l| l.grace).unwrap_or_else(|| grace_for(row));
        json.probe_timeout_effective_ms = grace.as_millis() as i64;
        if status == STATUS_STARTING {
            if let Some(l) = live.as_deref() {
                json.starting_left_ms = Some(grace.saturating_sub(l.started.elapsed()).as_millis() as i64);
            }
        }
    }
    if row.cmd != ps::CMD_APP_SENTINEL {
        json.reason = down_reason(row, status);
    }
    if row.kind == KIND_SKIN && status == STATUS_RUNNING {
        json.helper_needs_restart = helper_needs_restart(&row.id);
    }
    if row.kind != KIND_SKIN && status == STATUS_RUNNING {
        json.frame_warning = frame_warning_get(&row.project_id, &row.name);
    }
    // 0.46.3: the legacy label (a --cmd service no wall runs), and the App
    // serving its public label when its agent moved that over.
    if crate::apps::legacy::is_legacy(row) {
        json.legacy_label = Some(k2_core::app_grace::legacy_label());
        json.legacy_hint = Some(k2_core::app_grace::LEGACY_HINT.to_string());
        json.label_moved_to = crate::apps::labels::moved_to(&row.id);
    }
    // 0.46.16: a --skin site whose label an App took.
    if row.kind == KIND_SKIN {
        json.label_moved_to = crate::apps::labels::moved_to(&row.id);
    }
    if row.kind != KIND_SKIN {
        json.env_refused = crate::vault::publish_env::refused_refs(&row.id);
        json.clean_mode = crate::vault::publish_env::is_clean_mode(&row.id);
        if crate::vault::publish_env::is_legacy(&row.id) {
            json.env_mode = Some("legacy".into());
            json.env_removed_in = crate::vault::publish_env::LEGACY_ENV_REMOVED_IN.map(str::to_string);
            json.cutover_now = k2_core::app_grace::cutover_now();
            json.env_deprecation = Some(crate::vault::publish_env::legacy_env_notice(&row.name));
        } else {
            json.env_mode = Some("clean".into());
        }
    }
    json.visibility = visibility_json(row);
    // 0.46.18: every `--cmd` service, whatever its visibility: a public site
    // reached on its raw port still skips the server's front door. Not one
    // the owner marked public (`k2 publish expose <name> public`).
    let public = crate::publish_expose::public(&row.id);
    if public.is_none() {
        let exposed = exposed_binds(&row.id);
        if !exposed.is_empty() {
            json.bind_warning = Some(crate::publish_expose::warning(&row.name, row.port, &exposed));
        }
    }
    json.expose_public = public.map(|p| ps::ExposePublicJson { set_by: p.set_by, set_at: p.set_at });
    if row.dev_mem_mb.is_some() {
        json.dev = Some(ps::DevJson {
            idle_secs: dev_idle_limit(&row.name).map(|d| d.as_secs() as i64),
            idle_stopped_at: row.dev_idle_stopped_at,
            caps: crate::publish_dev::caps_of(pid),
            lane: crate::publish_dev::LANE_LABEL.to_string(),
            warning: dev_warning(json.visibility.as_ref()),
        });
    }
    json
}

/// What `ps` warns about a dev server: what it shows once someone other than
/// the owner can open it (DM-4), and this box's file-watch limit (DM-7).
fn dev_warning(visibility: Option<&ps::VisibilityJson>) -> Option<String> {
    let shared = visibility.filter(|v| v.mode != sv::MODE_PRIVATE).map(|_| crate::publish_dev::DEV_WARNING.to_string());
    let lines: Vec<String> = shared.into_iter().chain(crate::publish_dev::inotify_warning()).collect();
    (!lines.is_empty()).then(|| lines.join(" "))
}

/// The longest reason line `k2 publish ps` shows.
const REASON_MAX_CHARS: usize = 240;

/// The last non-empty line a service logged, redacted (vault values never
/// leave), at most [`REASON_MAX_CHARS`]. Reads only the log's last 8 KiB.
fn last_log_line(project_id: &str, name: &str) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = fs::File::open(log_path(project_id, name)).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(8 * 1024))).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let line = text.lines().rev().map(str::trim).find(|l| !l.is_empty())?;
    let line = crate::vault::redact_str(line);
    Some(if line.chars().count() > REASON_MAX_CHARS {
        format!("{}…", line.chars().take(REASON_MAX_CHARS).collect::<String>())
    } else {
        line
    })
}

/// Why a service isn't serving (K2 Cores field report 2026-10-28: a
/// service that exited at start showed only "stopped", and its public
/// address only "not accepting connections"): `exit 2: <its last log
/// line>`, or `not answering on port N: …` while it runs without
/// answering. `None` while it serves, starts, or after someone stopped it.
pub(crate) fn down_reason(row: &ps::PublishedService, status: &str) -> Option<String> {
    let failed_start = status == STATUS_STOPPED && row.error.is_some();
    let head = if status == STATUS_UNHEALTHY {
        format!("it runs but doesn't answer on port {}", row.port)
    } else if status == STATUS_EXITED || failed_start {
        // An exit code from before the last start is an older run's.
        let this_run = match (row.last_exited_at, row.last_started_at) {
            (Some(x), Some(s)) => x >= s,
            (Some(_), None) => true,
            _ => false,
        };
        match row.last_exit_code.filter(|_| this_run) {
            Some(c) => format!("its process exited with code {c}"),
            None => row.error.clone().unwrap_or_else(|| "not running".to_string()),
        }
    } else {
        return None;
    };
    Some(match last_log_line(&row.project_id, &row.name) {
        Some(l) => format!("{head}; its last log line: {l}"),
        None => head,
    })
}

fn load_row(project_id: &str, name: &str) -> Option<ps::PublishedService> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    ps::get_by_project_name(&conn, project_id, name).ok().flatten()
}

pub(crate) fn nested_preflight() -> Result<(tunnel_config::TunnelConfig, String), PublishError> {
    k2_core::airgap::refuse().map_err(PublishError::fail)?;
    let cfg = tunnel_config::load().map_err(PublishError::fail)?;
    // 0.45.7: a vault that can't be read right now is not "no token".
    if let Some(e) = cfg.token_read_error.as_deref() {
        return Err(PublishError::fail(format!(
            "token_unreadable: the K2 Connect tunnel token is in the {PRODUCT_NAME} vault, which could not be \
             read right now ({e}); nothing was published — try again once the vault is readable"
        )));
    }
    if cfg.token.trim().is_empty() {
        return Err(PublishError::fail(gate_hint("no_token")));
    }
    if !tunnel_config::e2e_enabled(&cfg) {
        return Err(PublishError::fail(gate_hint("e2e_off")));
    }
    let acct = subdomains::fetch_account(&cfg.subdomain, &cfg.token).map_err(|e| {
        if e.contains("no tunnel bearer") {
            PublishError::fail(gate_hint("no_token"))
        } else {
            PublishError::fail(e)
        }
    })?;
    if acct.tier.as_deref() != Some("pro") {
        return Err(PublishError::fail(gate_hint("pro_required")));
    }
    Ok((cfg, acct.tier.unwrap_or_default()))
}

fn gate_hint(kind: &str) -> String {
    format!(
        "{kind}: nested subdomains need a Pro plan (https://k2.dev/dashboard). \
         To host on this machine without a public URL: {CLI_NAME} publish run <name> --cmd \"…\" --port <n> --no-tunnel"
    )
}

/// The code an agent gets for an existing hostname its workspace doesn't
/// hold (the same as `apps::labels::LABEL_EXISTS`).
pub(crate) const AGENT_LABEL_EXISTS: &str = "label_exists_elsewhere";

/// `project_id` holds `label` (the 0074 attribution row).
fn agent_holds_label(project_id: &str, label: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    SubdomainWorkspace::owner_of(&conn, label).ok().flatten().as_deref() == Some(project_id)
}

pub(crate) fn attach_hostname(
    project_id: &str,
    name: &str,
    port: u16,
    token: &str,
    primary: &str,
    transfer: bool,
) -> Result<(), String> {
    let target = format!("127.0.0.1:{port}");
    // 0.46.3: belt for every caller (publish run, App open/deploy).
    let target_caller = crate::splice_guard::TargetCaller::from_label_caller(&crate::label_owner::current());
    crate::splice_guard::check_target(&target, target_caller).map_err(|r| r.to_string())?;
    // One owner per label: decide BEFORE any control-plane write, so a
    // label another workspace on this server holds is never repointed.
    let caller = crate::label_owner::current();
    let decision = crate::label_owner::check(name, project_id, transfer).map_err(|r| r.message())?;
    match subdomains::create_subdomain(token, name, &target) {
        Ok(()) => {}
        // Already on the account: repoint only because `decision` says
        // this workspace holds it (or it is unattributed, or an audited
        // owner/admin transfer).
        Err(e) if e.contains("label_taken") => {
            // The control plane says the label exists. An agent repoints it
            // only when its own workspace holds it: never on the word of a
            // cached hostname map that is empty or stale (security review D).
            if caller == crate::label_owner::Caller::Agent && !agent_holds_label(project_id, name) {
                return Err(format!(
                    "{AGENT_LABEL_EXISTS}: '{name}' already exists on this box and isn't your workspace's: an agent never repoints it. \
                     Pick a new label, or ask your owner to move it deliberately (--transfer, from a terminal outside {PRODUCT_NAME})"
                ));
            }
            subdomains::point_subdomain(token, name, &target)?;
        }
        Err(e) => return Err(e),
    }
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        crate::label_owner::apply(&conn, name, project_id, None, &decision, &caller)
            .map_err(|r| r.message())?;
    }
    crate::session_events::emit_tunnel_subdomains_changed();
    let tries = claim_tries();
    for i in 0..tries {
        match subdomains::refresh_once(primary, token) {
            Ok(_) => {}
            Err(e) => {
                if i + 1 == tries {
                    return Err(format!("hostname refresh failed: {e}"));
                }
            }
        }
        let (_, targets) = crate::session_events::tunnel_subdomains_snapshot();
        if let Some(t) = targets.get(name) {
            if t.project_id.as_deref() == Some(project_id) {
                return Ok(());
            }
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err("hostname step failed: nested label did not appear in the subdomain snapshot".into())
}

fn adopt_live(
    project_id: &str,
    name: &str,
    port: u16,
    child: Child,
    pid: i32,
    job: Option<WinJobSlot>,
    grace: Duration,
) -> Arc<LiveChild> {
    let entry = Arc::new(LiveChild {
        project_id: project_id.to_string(),
        name: name.to_string(),
        pid,
        port,
        started: Instant::now(),
        grace,
        probe_ok: AtomicBool::new(false),
        stop_requested: AtomicBool::new(false),
        child: Mutex::new(Some(child)),
        #[cfg(windows)]
        job: Mutex::new(job),
    });
    #[cfg(not(windows))]
    let _ = job;
    live_insert(Arc::clone(&entry));
    let watcher = Arc::clone(&entry);
    thread::Builder::new()
        .name(format!("publish-wait-{name}"))
        .spawn(move || wait_child(watcher))
        .ok();
    entry
}

fn adopt_reattach(project_id: &str, name: &str, port: u16, pid: i32) -> Arc<LiveChild> {
    let entry = Arc::new(LiveChild {
        project_id: project_id.to_string(),
        name: name.to_string(),
        pid,
        port,
        started: Instant::now(),
        grace: probe_timeout(),
        probe_ok: AtomicBool::new(true),
        stop_requested: AtomicBool::new(false),
        child: Mutex::new(None),
        #[cfg(windows)]
        job: Mutex::new(None),
    });
    live_insert(Arc::clone(&entry));
    let watcher = Arc::clone(&entry);
    thread::Builder::new()
        .name(format!("publish-poll-{name}"))
        .spawn(move || poll_foreign(watcher))
        .ok();
    entry
}

fn wait_child(entry: Arc<LiveChild>) {
    let log = log_path(&entry.project_id, &entry.name);
    let mut ticks = 0u32;
    loop {
        ticks = ticks.wrapping_add(1);
        if ticks % 25 == 0 {
            rotate_in_place_if_needed(&log);
        }
        let status = {
            let mut g = entry.child.lock().unwrap_or_else(|e| e.into_inner());
            match g.as_mut() {
                Some(child) => match child.try_wait() {
                    Ok(Some(st)) => Some(st.code().unwrap_or(1) as i64),
                    Ok(None) => None,
                    Err(_) => Some(1),
                },
                None => {
                    if !pid_alive(entry.pid) {
                        Some(entry_exit_fallback())
                    } else {
                        None
                    }
                }
            }
        };
        if let Some(code) = status {
            on_child_exit(&entry, code);
            return;
        }
        thread::sleep(Duration::from_millis(200));
    }
}

fn entry_exit_fallback() -> i64 {
    1
}

fn poll_foreign(entry: Arc<LiveChild>) {
    let log = log_path(&entry.project_id, &entry.name);
    let mut ticks = 0u32;
    loop {
        ticks = ticks.wrapping_add(1);
        if ticks % 10 == 0 {
            rotate_in_place_if_needed(&log);
        }
        if entry.stop_requested.load(Ordering::SeqCst) {
            return;
        }
        if !pid_alive(entry.pid) {
            on_child_exit(&entry, 1);
            return;
        }
        thread::sleep(Duration::from_millis(500));
    }
}

fn on_child_exit(entry: &LiveChild, code: i64) {
    // A restart (`k2 publish frame`, boot drift) may already have put a new
    // child in this slot; only this entry's own slot and pid are cleared.
    {
        let mut map = live().lock().unwrap_or_else(|e| e.into_inner());
        let key = (entry.project_id.clone(), entry.name.clone());
        if map.get(&key).is_some_and(|cur| std::ptr::eq(cur.as_ref(), entry)) {
            map.remove(&key);
        }
    }
    {
        // A stop K2 asked for (a person's, an idle stop) is no crash: its exit
        // code isn't recorded. On Windows the job ends at once and this
        // watcher can land after the stop's own record, so it must not
        // stamp the terminate's code 1 over it.
        let recorded = (!entry.stop_requested.load(Ordering::SeqCst)).then_some(code);
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = ps::mark_exited_if_pid(&conn, &entry.project_id, &entry.name, entry.pid as i64, recorded);
    }
    if !entry.stop_requested.load(Ordering::SeqCst) {
        log_debug!(
            "[publish] {}/{} pid={} exited code={} (desired unchanged)",
            entry.project_id,
            entry.name,
            entry.pid,
            code
        );
    }
    emit_changed(&entry.project_id);
}

fn is_running_now(project_id: &str, name: &str, row: &ps::PublishedService) -> bool {
    if live_get(project_id, name).is_some() {
        return true;
    }
    if let Some(pid) = row.pid.and_then(|p| i32::try_from(p).ok()) {
        if pid_alive(pid) && port_accepts(row.port as u16) {
            return true;
        }
    }
    false
}

pub struct RunSpec {
    pub project_id: String,
    pub name: String,
    pub cmd: String,
    pub cwd: PathBuf,
    pub port: u16,
    pub no_tunnel: bool,
    pub replace_spec: bool,
    pub skin: bool,
    pub skin_root: String,
    /// True when the caller passed `--cwd` / `cwd=` (forbidden with `--skin`).
    pub cwd_explicit: bool,
    /// Explicit owner/admin transfer of a label another workspace holds
    /// (`--transfer`). Refused for agents and members; audited.
    pub transfer: bool,
    /// `--probe-timeout`: the startup grace stored on the row.
    pub probe_timeout: ProbeTimeout,
    /// `--dev` (DM-1): a dev server from the live folder. `None` turns a dev
    /// service back into a normal one.
    pub dev: Option<crate::publish_dev::DevSpec>,
}

/// What a run does to the stored startup grace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProbeTimeout {
    /// No flag: a new service gets the default, a re-run keeps its value.
    #[default]
    Keep,
    /// `--probe-timeout default`: back to the daemon default.
    Default,
    /// `--probe-timeout <dur>`, in ms (1 ms to 15 min).
    Ms(u64),
}

pub fn run(spec: RunSpec) -> Result<ServiceJson, PublishError> {
    let name = ps::normalize_existing_name(&spec.name).map_err(PublishError::bad)?;
    let existing = load_row(&spec.project_id, &name);
    // App runtime rows are driven by `k2 app`, never by `k2 publish run`.
    if let Some(row) = &existing {
        refuse_app_row(row)?;
    }
    // Reserved nested labels (`meet`, `chat`): only a re-run of a row this
    // workspace already had before the reservation goes through. A new
    // row under a reserved name is refused before anything changes.
    if k2_core::skin::is_reserved_nested_label(&name) && existing.is_none() {
        return Err(PublishError::bad(k2_core::skin::reserved_nested_label_error(&name)));
    }
    if spec.skin && spec.cwd_explicit {
        return Err(PublishError::bad(
            "--cwd and --skin are mutually exclusive",
        ));
    }
    if spec.skin && !spec.cmd.trim().is_empty() {
        return Err(PublishError::bad(
            "--cmd and --skin are mutually exclusive",
        ));
    }
    if !spec.skin && spec.cmd.trim().is_empty() {
        return Err(PublishError::bad("Missing cmd"));
    }
    if spec.dev.is_some() {
        if spec.skin {
            return Err(PublishError::coded(
                "dev_needs_cmd",
                "--dev runs a dev server, so it needs --cmd (--skin hosts static files)",
            ));
        }
        if spec.no_tunnel {
            return Err(PublishError::coded(
                "dev_needs_tunnel",
                concat!("--dev puts a dev server behind ", product_name!(), "'s check at its address, so it can't use --no-tunnel. To run one for yourself only, run the command in a terminal"),
            ));
        }
        // PV-4 / G8: where K2's check can't see the traffic, a dev server would
        // be open to anyone. So it doesn't start.
        if let Some(e) = crate::site_gate::door_refusal(&name) {
            let (code, words) = e.split_once(": ").unwrap_or((sv::DOOR_UNSUPPORTED, e.as_str()));
            return Err(PublishError::coded(code, words));
        }
    }
    if spec.port == 0 {
        return Err(PublishError::bad("Missing port"));
    }
    // 0.46.3: never this daemon's own port (the published address would
    // route public traffic into the daemon as if it were local).
    let caller = crate::splice_guard::TargetCaller::from_label_caller(&crate::label_owner::current());
    crate::splice_guard::check_target(&format!("127.0.0.1:{}", spec.port), caller)
        .map_err(PublishError::target)?;
    if let ProbeTimeout::Ms(ms) = spec.probe_timeout {
        if ms == 0 || ms > ps::PROBE_TIMEOUT_MAX_MS {
            return Err(PublishError::bad(
                "--probe-timeout must be more than 0 and at most 15m",
            ));
        }
    }
    let skin_root = if spec.skin {
        validate_skin_root(&spec.cwd, &spec.skin_root).map_err(PublishError::bad)?
    } else {
        String::new()
    };
    let cmd = if spec.skin {
        CMD_SKIN_SENTINEL.to_string()
    } else {
        spec.cmd.trim().to_string()
    };
    let kind = if spec.skin { KIND_SKIN } else { KIND_CMD };
    if !spec.cwd.is_dir() {
        return Err(PublishError::bad(format!(
            "cwd is not a directory: {}",
            spec.cwd.display()
        )));
    }
    let expose = if spec.no_tunnel {
        EXPOSE_LOCAL
    } else {
        EXPOSE_TUNNEL
    };
    if expose == EXPOSE_TUNNEL {
        // Refuse a label another workspace holds before any row, process
        // or control-plane call (structured 409). Local and cheap, so it
        // runs ahead of the Connect plan preflight.
        crate::label_owner::check(&name, &spec.project_id, spec.transfer)?;
        nested_preflight()?;
    }

    let existing = load_row(&spec.project_id, &name);
    if let Some(ref row) = existing {
        if is_running_now(&spec.project_id, &name, row) {
            return Err(PublishError::bad(
                "service is already running — stop first",
            ));
        }
    }

    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if existing.is_some() {
            let _ = ps::update_spec(
                &conn,
                &spec.project_id,
                &name,
                &cmd,
                &spec.cwd.to_string_lossy(),
                spec.port,
                expose,
                kind,
                &skin_root,
            );
            let _ = ps::set_desired(&conn, &spec.project_id, &name, DESIRED_STOPPED);
            let _ = ps::set_error(&conn, &spec.project_id, &name, None);
        } else {
            ps::insert(
                &conn,
                &spec.project_id,
                &name,
                &cmd,
                &spec.cwd.to_string_lossy(),
                spec.port,
                expose,
                DESIRED_STOPPED,
                kind,
                &skin_root,
            )
            .map_err(|e| {
                if ps::is_unique_violation(&e) {
                    PublishError::bad("service name already exists")
                } else {
                    PublishError::fail(e.to_string())
                }
            })?;
        }
        let stored = match spec.probe_timeout {
            ProbeTimeout::Keep => None,
            ProbeTimeout::Default => Some(None),
            ProbeTimeout::Ms(ms) => Some(Some(ms)),
        };
        if let Some(ms) = stored {
            ps::set_probe_timeout(&conn, &spec.project_id, &name, ms)
                .map_err(|e| PublishError::fail(e.to_string()))?;
        }
        ps::set_dev_caps(&conn, &spec.project_id, &name, spec.dev.map(|d| (d.mem_mb, d.cpus)))
            .map_err(|e| PublishError::fail(e.to_string()))?;
    }
    // The gate reads this before the hostname exists, so a dev site is never
    // reachable as anything but what it is.
    sync_site_visibility(&name, expose == EXPOSE_TUNNEL, spec.dev.as_ref())?;
    if spec.dev.is_none() {
        crate::publish_dev::forget(&name);
    }

    match start_inner(&spec.project_id, &name, expose == EXPOSE_TUNNEL, spec.transfer) {
        Ok(json) => Ok(json),
        Err(e) => {
            if expose == EXPOSE_TUNNEL {
                // Nested run that never got to desired=running: drop the row
                // if we just inserted it and never started, else leave stopped+error.
                let db = k2_core::db::shared();
                let conn = db.lock();
                if let Some(row) = ps::get_by_project_name(&conn, &spec.project_id, &name).ok().flatten() {
                    if row.pid.is_none() && row.last_started_at.is_none() && !spec.replace_spec {
                        let _ = ps::delete(&conn, &spec.project_id, &name);
                    }
                }
            }
            Err(e)
        }
    }
}

/// App runtime rows are driven by `k2 app`, never by the publish verbs.
fn refuse_app_row(row: &ps::PublishedService) -> Result<(), PublishError> {
    if row.cmd == ps::CMD_APP_SENTINEL {
        return Err(PublishError::bad(format!(
            "{}: '{}' is an App; use {CLI_NAME} app start|stop|deploy|uninstall {}",
            k2_core::deploy::codes::IS_AN_APP,
            row.name,
            row.name
        )));
    }
    Ok(())
}

pub fn start(project_id: &str, name: &str) -> Result<ServiceJson, PublishError> {
    let name = ps::normalize_existing_name(name).map_err(PublishError::bad)?;
    let row = load_row(project_id, &name).ok_or_else(|| PublishError::bad("no such published service"))?;
    refuse_app_row(&row)?;
    if is_running_now(project_id, &name, &row) {
        return Ok(to_json(&row));
    }
    if row.expose == EXPOSE_TUNNEL {
        crate::label_owner::check(&name, project_id, false)?;
        nested_preflight()?;
    }
    start_inner(project_id, &name, row.expose == EXPOSE_TUNNEL, false)
}

fn start_inner(
    project_id: &str,
    name: &str,
    want_tunnel: bool,
    transfer: bool,
) -> Result<ServiceJson, PublishError> {
    let row = load_row(project_id, name).ok_or_else(|| PublishError::bad("no such published service"))?;
    if port_accepts(row.port as u16) {
        if let Some(pid) = row.pid.and_then(|p| i32::try_from(p).ok()) {
            if pid_alive(pid) {
                adopt_reattach(project_id, name, row.port as u16, pid);
                {
                    let db = k2_core::db::shared();
                    let conn = db.lock();
                    let _ = ps::set_desired(&conn, project_id, name, DESIRED_RUNNING);
                }
                emit_changed(project_id);
                let row = load_row(project_id, name).unwrap_or(row);
                return Ok(to_json(&row));
            }
        }
        return Err(PublishError::fail(format!(
            "bind clash: 127.0.0.1:{} is already accepting connections",
            row.port
        )));
    }

    // M2 review F5: every start (`publish start`, the boot relaunch, an
    // approved re-run) checks the stored folder again for a fenced
    // workspace's service: a folder that became a link out since it was
    // published is refused, not followed.
    if crate::agent_isolation::workspace_and_fence(project_id).1 {
        let mut folders = vec![PathBuf::from(&row.cwd)];
        if !row.skin_root.trim().is_empty() {
            folders.push(PathBuf::from(&row.cwd).join(row.skin_root.trim()));
        }
        if let Some(refusal) = crate::agent_isolation::reach_refusal(project_id, &folders, "start") {
            let why = serde_json::from_str::<serde_json::Value>(&refusal.body)
                .ok()
                .and_then(|v| v["error"]["hint"].as_str().map(str::to_string))
                .unwrap_or_else(|| "the service's folder is outside the agent's reach".into());
            return Err(PublishError::bad(why));
        }
    }
    let cwd = PathBuf::from(&row.cwd);
    let (child, pid, job) = if row.kind == KIND_SKIN {
        spawn_skin_helper(&row, project_id, name).map_err(PublishError::fail)?
    } else {
        let dev = row.dev_mem_mb.map(|mem| (mem, row.dev_cpus.unwrap_or(crate::publish_dev::CPUS_DEFAULT)));
        spawn_child(&row.cmd, &cwd, project_id, name, Some((&row.id, row.port as u16)), dev).map_err(PublishError::fail)?
    };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = ps::set_runtime(&conn, project_id, name, Some(pid as i64), None, Some(now_unix()));
        if row.dev_idle_stopped_at.is_some() {
            let _ = ps::set_dev_idle_stopped(&conn, project_id, name, None);
        }
    }
    let grace = grace_for(&row);
    let entry = adopt_live(project_id, name, row.port as u16, child, pid, job, grace);
    emit_changed(project_id);

    // The request waits at most the default window (15 s); a longer
    // `--probe-timeout` goes on in the background (status `starting`).
    let window = grace.min(probe_timeout());
    if !wait_for_port(row.port as u16, window) {
        entry.probe_ok.store(false, Ordering::SeqCst);
        let alive = matches!(slot_state(&entry), Slot::Current);
        if alive && grace > window {
            // Inside the grace. desired=running so boot honours it; a
            // tunnelled service's hostname waits for the first answer.
            {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let _ = ps::set_desired(&conn, project_id, name, DESIRED_RUNNING);
                let _ = ps::set_error(&conn, project_id, name, None);
                let _ = ps::set_hostname_pending(&conn, project_id, name, want_tunnel);
            }
            spawn_grace_waiter(Arc::clone(&entry), want_tunnel, transfer);
            emit_changed(project_id);
            let row = load_row(project_id, name).unwrap_or(row);
            return Ok(to_json(&row));
        }
        if want_tunnel {
            let msg = if alive {
                grace_miss_message(row.port as u16, grace)
            } else {
                format!(
                    "probe failed: the process exited before 127.0.0.1:{} accepted ({CLI_NAME} publish logs {name})",
                    row.port
                )
            };
            rollback_hostname_fail(project_id, name, &entry, &msg);
            return Err(PublishError::fail(msg));
        }
        // Local: keep the process (status unhealthy). Persist desired=running
        // so boot still honors it, and keep probing so a late bind turns
        // it `running`.
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = ps::set_desired(&conn, project_id, name, DESIRED_RUNNING);
        }
        if alive {
            spawn_grace_waiter(Arc::clone(&entry), false, false);
        }
        emit_changed(project_id);
        let row = load_row(project_id, name).unwrap_or(row);
        return Ok(to_json(&row));
    }
    entry.probe_ok.store(true, Ordering::SeqCst);
    finish_start(project_id, name, &entry, &row, want_tunnel, transfer)?;
    let row = load_row(project_id, name).unwrap_or(row);
    Ok(to_json(&row))
}

/// The port answered: publish the nested hostname (tunnel), then mark the
/// service running. A hostname failure rolls the start back.
fn finish_start(
    project_id: &str,
    name: &str,
    entry: &LiveChild,
    row: &ps::PublishedService,
    want_tunnel: bool,
    transfer: bool,
) -> Result<(), PublishError> {
    if want_tunnel {
        let cfg = tunnel_config::load().map_err(|e| {
            rollback_hostname_fail(project_id, name, entry, &e);
            PublishError::fail(e)
        })?;
        if let Err(e) = attach_hostname(
            project_id,
            name,
            row.port as u16,
            &cfg.token,
            &cfg.subdomain,
            transfer,
        ) {
            rollback_hostname_fail(project_id, name, entry, &e);
            return Err(PublishError::fail(e));
        }
    }

    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = ps::set_desired(&conn, project_id, name, DESIRED_RUNNING);
        let _ = ps::set_error(&conn, project_id, name, None);
        let _ = ps::set_hostname_pending(&conn, project_id, name, false);
    }
    if row.kind != KIND_SKIN {
        probe_cmd_frame_async(project_id, name, row.port as u16);
        bind_check_async(project_id, name);
    }
    emit_changed(project_id);
    Ok(())
}

/// Where a live entry stands in the live map.
enum Slot {
    /// Still this service's child.
    Current,
    /// A restart put another child in its place.
    Replaced,
    /// Gone: the child exited or was stopped.
    Gone,
}

fn slot_state(entry: &LiveChild) -> Slot {
    let map = live().lock().unwrap_or_else(|e| e.into_inner());
    match map.get(&(entry.project_id.clone(), entry.name.clone())) {
        Some(cur) if std::ptr::eq(cur.as_ref(), entry) => Slot::Current,
        Some(_) => Slot::Replaced,
        None => Slot::Gone,
    }
}

fn spawn_grace_waiter(entry: Arc<LiveChild>, want_tunnel: bool, transfer: bool) {
    let name = entry.name.clone();
    let _ = thread::Builder::new()
        .name(format!("publish-grace-{name}"))
        .spawn(move || grace_wait(entry, want_tunnel, transfer));
}

/// Background re-probe after the start request returned. Every
/// [`reprobe_interval`] inside the grace: port open → [`finish_start`]
/// (`starting` → `running`, hostname published, changed event). Grace
/// over: a tunnelled service is rolled back exactly like a sync miss; a
/// local one turns `unhealthy` and is re-probed every
/// [`supervise_interval`] until it binds, exits or is stopped.
fn grace_wait(entry: Arc<LiveChild>, want_tunnel: bool, transfer: bool) {
    let (project_id, name) = (entry.project_id.clone(), entry.name.clone());
    let mut told_unhealthy = false;
    loop {
        if entry.stop_requested.load(Ordering::SeqCst) {
            return;
        }
        match slot_state(&entry) {
            Slot::Current => {}
            Slot::Replaced => return,
            Slot::Gone => {
                // `stop` removes the slot a moment before it flags the
                // entry; give it that moment.
                thread::sleep(Duration::from_millis(50));
                if entry.stop_requested.load(Ordering::SeqCst) {
                    return;
                }
                if want_tunnel {
                    {
                        let db = k2_core::db::shared();
                        let conn = db.lock();
                        let _ = ps::mark_hostname_failed(
                            &conn,
                            &project_id,
                            &name,
                            &format!(
                                "probe failed: the process exited before 127.0.0.1:{} accepted ({CLI_NAME} publish logs {name})",
                                entry.port
                            ),
                        );
                    }
                    emit_changed(&project_id);
                }
                return;
            }
        }
        if port_accepts(entry.port) {
            entry.probe_ok.store(true, Ordering::SeqCst);
            let Some(row) = load_row(&project_id, &name) else {
                return;
            };
            match finish_start(&project_id, &name, &entry, &row, want_tunnel, transfer) {
                Ok(()) => log_debug!(
                    "[publish] {project_id}/{name} port {} answered after {}",
                    entry.port,
                    fmt_duration(entry.started.elapsed())
                ),
                Err(e) => log_debug!("[publish] {project_id}/{name} late start failed: {}", e.message),
            }
            return;
        }
        let elapsed = entry.started.elapsed();
        if elapsed >= entry.grace {
            if want_tunnel {
                let msg = grace_miss_message(entry.port, entry.grace);
                log_debug!("[publish] {project_id}/{name} {msg}");
                rollback_hostname_fail(&project_id, &name, &entry, &msg);
                return;
            }
            if !told_unhealthy {
                told_unhealthy = true;
                emit_changed(&project_id);
            }
            thread::sleep(supervise_interval());
        } else {
            thread::sleep(
                reprobe_interval()
                    .min(entry.grace - elapsed)
                    .max(Duration::from_millis(20)),
            );
        }
    }
}

fn rollback_hostname_fail(project_id: &str, name: &str, entry: &LiveChild, error: &str) {
    kill_tree(entry);
    live_remove(project_id, name);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = ps::mark_hostname_failed(&conn, project_id, name, error);
    }
    emit_changed(project_id);
}

pub fn stop(project_id: &str, name: &str) -> Result<ServiceJson, PublishError> {
    let name = ps::normalize_existing_name(name).map_err(PublishError::bad)?;
    let row = load_row(project_id, &name).ok_or_else(|| PublishError::bad("no such published service"))?;
    refuse_app_row(&row)?;
    if let Some(entry) = live_remove(project_id, &name) {
        if let Err(e) = kill_tree_checked(&entry) {
            // Windows: part of it outlived the cap. The owner asked for a
            // stop, so it isn't restarted; the pid stays on the row, so the
            // next stop ends that tree by pid. Never reported as stopped.
            {
                let db = k2_core::db::shared();
                let conn = db.lock();
                let _ = ps::set_desired(&conn, project_id, &name, DESIRED_STOPPED);
            }
            emit_changed(project_id);
            return Err(PublishError::fail(format!("the stop didn't finish: {e}")));
        }
    } else if let Some(pid) = row.pid.and_then(|p| i32::try_from(p).ok()) {
        // Reattached / unknown live pid: kill the group we own if pgid==pid.
        kill_pid_tree(pid);
    }
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = ps::set_desired(&conn, project_id, &name, DESIRED_STOPPED);
        let _ = ps::set_hostname_pending(&conn, project_id, &name, false);
        let _ = ps::mark_exited(&conn, project_id, &name, None);
        let _ = ps::set_runtime(&conn, project_id, &name, None, row.error.as_deref(), None);
        // A person's stop outranks an idle stop: it doesn't wake on a visit.
        let _ = ps::set_dev_idle_stopped(&conn, project_id, &name, None);
    }
    crate::publish_dev::forget(&name);
    emit_changed(project_id);
    let row = load_row(project_id, &name).unwrap_or(row);
    Ok(to_json(&row))
}

// ── dev servers: idle stop, wake, bind check (DM-6, PA-11, PV-4) ─────────

/// A dev label's idle limit, from its visibility row. `None` = `--idle off`.
fn dev_idle_limit(label: &str) -> Option<Duration> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let idle = sv::effective(&conn, label).ok().and_then(|v| v.idle_secs);
    match idle {
        None => Some(Duration::from_secs(crate::publish_dev::IDLE_DEFAULT_SECS as u64)),
        Some(0) => None,
        Some(n) => Some(Duration::from_secs(n.max(0) as u64)),
    }
}

/// What the gate reads for this run, written before the hostname exists (the
/// label's own private row comes with the hostname's creation):
/// - a dev run marks it dev with its idle limit, and takes a public site private
///   unless it was already a dev site someone made public on purpose (G8);
/// - a plain run of a dev site turns it back into a normal one (DM-1).
fn sync_site_visibility(label: &str, tunnel: bool, dev: Option<&crate::publish_dev::DevSpec>) -> Result<(), PublishError> {
    if !tunnel {
        return Ok(());
    }
    const BY: &str = concat!(cli_name!(), " publish run");
    let db = k2_core::db::shared();
    let conn = db.lock();
    let fail = |e: String| PublishError::fail(e);
    let current = sv::effective(&conn, label).map_err(fail)?;
    match dev {
        Some(d) => {
            if current.mode == sv::Mode::Public && !current.dev {
                sv::set_private(&conn, label, BY).map_err(fail)?;
            }
            sv::set_dev(&conn, label, true, Some(d.idle_secs.unwrap_or(0)), BY).map_err(fail)?;
        }
        None if current.dev => {
            sv::set_dev(&conn, label, false, None, BY).map_err(fail)?;
        }
        None => {}
    }
    Ok(())
}

/// Who may open this service's site, for `ps`. `None`: it has no address, or an
/// App's own logins decide (a folder App has none: the gate covers it).
fn visibility_json(row: &ps::PublishedService) -> Option<ps::VisibilityJson> {
    if row.expose != EXPOSE_TUNNEL || !sv::is_covered(&row.name) || crate::publish_site_routes::is_real_app(&row.name) {
        return None;
    }
    let v = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        sv::effective(&conn, &row.name).ok()?
    };
    Some(ps::VisibilityJson {
        mode: v.mode.as_str().to_string(),
        dev: v.dev,
        // Where K2's check can't see the traffic, say so rather than "private".
        enforced: v.mode == sv::Mode::Public || crate::site_gate::door_refusal(&row.name).is_none(),
        reason: v.public_reason,
        set_by: v.set_by,
        set_at: v.set_at,
    })
}

fn bind_noticed() -> &'static Mutex<std::collections::HashSet<(String, Vec<String>)>> {
    static SEEN: OnceLock<Mutex<std::collections::HashSet<(String, Vec<String>)>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

fn workspace_path(project_id: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT path FROM projects WHERE id = ?1", rusqlite::params![project_id], |r| r.get(0)).unwrap_or_else(|_| project_id.to_string())
}

/// A `--cmd` service that listens beyond loopback (0.46.18: any, not only a
/// gated site), once per set of addresses in this daemon's life: a line in
/// the daemon's log and in the service's own log (`k2 publish logs`). A private
/// or PIN site also gets one Thread notice (PV-4). Never for a service the
/// owner marked public, and never a stop: a deliberately open one keeps
/// working.
fn notify_bind_exposed(row: &ps::PublishedService, exposed: &[String]) {
    if exposed.is_empty() || crate::publish_expose::public(&row.id).is_some() {
        return;
    }
    if !bind_noticed().lock().unwrap_or_else(|e| e.into_inner()).insert((row.id.clone(), exposed.to_vec())) {
        return;
    }
    let warning = crate::publish_expose::warning(&row.name, row.port, exposed);
    log_debug!("[publish] {}: {warning}", row.name);
    note_in_service_log(&row.project_id, &row.name, &format!("[k2] warning: {warning}"));
    let gated = visibility_json(row).is_some_and(|v| v.mode != sv::MODE_PUBLIC);
    if !gated {
        return;
    }
    let text = crate::publish_dev::bind_warning(&row.name, exposed);
    if let Err(e) = crate::overlay_routes::post_workspace_note(&workspace_path(&row.project_id), &text, "k2-publish", k2_core::overlay::SystemNotice::action("publish.bind_exposed")) {
        log_debug!("[publish] {}: bind notice: {e}", row.name);
    }
}

/// One line of K2's own in a service's log, beside what its program writes.
fn note_in_service_log(project_id: &str, name: &str, line: &str) {
    match open_log(project_id, name) {
        Ok(mut f) => {
            if let Err(e) = writeln!(f, "{line}") {
                log_debug!("[publish] {name}: log note: {e}");
            }
        }
        Err(e) => log_debug!("[publish] {name}: log note: {e}"),
    }
}

/// `k2 vault doctor`'s finding (0.46.18): each `--cmd` service the last bind
/// check saw listening beyond this server, `(name, warning)`. `workspace`:
/// only that workspace's (an agent's doctor).
pub(crate) fn bind_doctor_findings(workspace: Option<&str>) -> Vec<(String, String)> {
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::list_all(&conn).unwrap_or_default()
    };
    rows.iter()
        .filter(|r| workspace.is_none_or(|w| w == r.project_id))
        .filter(|r| crate::publish_expose::public(&r.id).is_none())
        .filter_map(|r| {
            let exposed = exposed_binds(&r.id);
            (!exposed.is_empty()).then(|| (r.name.clone(), crate::publish_expose::warning(&r.name, r.port, &exposed)))
        })
        .collect()
}

/// PV-11: one card about the sites public from before 0.46.7, once the upgrade's
/// pass over the routing map has settled which those are.
fn upgrade_card_tick() {
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if !sv::upgrade_card_due(&conn).unwrap_or(false) {
            return;
        }
    }
    if crate::publish_cards::present_upgrade_card() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Err(e) = sv::mark_upgrade_card_done(&conn) {
            log_debug!("[publish] upgrade card mark: {e}");
        }
    }
}

/// Something came through the front door for this dev label. Cheap: the gate
/// calls it per request and while a spliced WebSocket carries data, so a person
/// watching hot reload isn't idle-stopped mid-session.
pub fn dev_touch(label: &str) {
    crate::publish_dev::touch(&label.trim().to_ascii_lowercase());
}

/// The row serving `label`: the workspace that holds the label, else the only
/// workspace with a service of that name. Two unattributed rows: none (never a
/// guess at which dev server to start).
fn service_for_label(label: &str) -> Option<ps::PublishedService> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Ok(Some(owner)) = SubdomainWorkspace::owner_of(&conn, label) {
        return ps::get_by_project_name(&conn, &owner, label).ok().flatten();
    }
    let mut rows = ps::list_all(&conn).ok()?.into_iter().filter(|r| r.name == label);
    let first = rows.next()?;
    rows.next().is_none().then_some(first)
}

fn waking() -> &'static Mutex<std::collections::HashSet<String>> {
    static WAKING: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    WAKING.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

/// DM-6: called by the gate for a visitor who passed it, before dialing. It
/// never blocks. Only an idle-stopped dev server wakes: a stopped or crashed
/// one stays down until someone starts it, so reloads can't make a crash loop.
pub fn dev_wake(label: &str) -> DevWake {
    let label = label.trim().to_ascii_lowercase();
    let Some(row) = service_for_label(&label) else { return DevWake::NotDev };
    if row.dev_mem_mb.is_none() {
        return DevWake::NotDev;
    }
    if live_get(&row.project_id, &row.name).is_some() {
        crate::publish_dev::touch(&label);
        return if port_accepts(row.port as u16) { DevWake::Up } else { DevWake::Starting };
    }
    if waking().lock().unwrap_or_else(|e| e.into_inner()).contains(&label) {
        return DevWake::Starting;
    }
    if row.dev_idle_stopped_at.is_none() {
        if row.pid.and_then(|p| i32::try_from(p).ok()).is_some_and(pid_alive) && port_accepts(row.port as u16) {
            return DevWake::Up;
        }
        let why = row.error.clone().unwrap_or_else(|| {
            if row.desired == DESIRED_STOPPED {
                concat!("This dev server is stopped. Its owner can start it: ", cli_name!(), " publish start").to_string()
            } else {
                concat!("This dev server exited. Its owner can see why: ", cli_name!(), " publish logs").to_string()
            }
        });
        return DevWake::Failed(why);
    }
    if !waking().lock().unwrap_or_else(|e| e.into_inner()).insert(label.clone()) {
        return DevWake::Starting;
    }
    crate::publish_dev::touch(&label);
    let spawned = thread::Builder::new().name(format!("publish-wake-{label}")).spawn({
        let label = label.clone();
        move || {
            let want_tunnel = row.expose == EXPOSE_TUNNEL;
            let r = start_inner(&row.project_id, &row.name, want_tunnel, false);
            if let Err(e) = r {
                log_debug!("[publish] {label}: wake failed: {}", e.message);
                let db = k2_core::db::shared();
                let conn = db.lock();
                let _ = ps::set_error(&conn, &row.project_id, &row.name, Some(&e.message));
                let _ = ps::set_dev_idle_stopped(&conn, &row.project_id, &row.name, None);
            }
            waking().lock().unwrap_or_else(|e| e.into_inner()).remove(&label);
        }
    });
    if let Err(e) = spawned {
        waking().lock().unwrap_or_else(|p| p.into_inner()).remove(&label);
        return DevWake::Failed(format!("{PRODUCT_NAME} couldn't start the dev server: {e}"));
    }
    DevWake::Starting
}

/// Stop every dev server that has been idle past its limit. `now` is passed in
/// so tests needn't wait out the 5-minute minimum.
pub(crate) fn dev_idle_sweep(now: Instant) -> Vec<String> {
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::list_desired_running(&conn).unwrap_or_default()
    };
    let mut stopped = Vec::new();
    for row in rows.into_iter().filter(|r| r.dev_mem_mb.is_some()) {
        let Some(limit) = dev_idle_limit(&row.name) else { continue };
        let Some(entry) = live_get(&row.project_id, &row.name) else { continue };
        if !crate::publish_dev::idle_due(entry.started, crate::publish_dev::last_touch(&row.name), limit, now) {
            continue;
        }
        if let Err(e) = idle_stop(&row.project_id, &row.name, limit) {
            log_debug!("[publish] {}: idle stop failed: {}", row.name, e.message);
            continue;
        }
        stopped.push(row.name);
    }
    stopped
}

fn idle_stop(project_id: &str, name: &str, limit: Duration) -> Result<(), PublishError> {
    stop(project_id, name)?;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::set_dev_idle_stopped(&conn, project_id, name, Some(now_unix())).map_err(|e| PublishError::fail(e.to_string()))?;
    }
    log_debug!("[publish] {project_id}/{name}: idle-stopped after {} with no visits", fmt_duration(limit));
    emit_changed(project_id);
    Ok(())
}

/// A site's visibility changed: every workspace showing a service of that name
/// refreshes its list.
pub(crate) fn emit_label_changed(label: &str) {
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::list_all(&conn).unwrap_or_default()
    };
    for r in rows.iter().filter(|r| r.name == label) {
        emit_changed(&r.project_id);
    }
}

/// The running dev site whose live folder is `folder`, if any (DM-8).
pub(crate) fn dev_site_in_folder(folder: &str) -> Option<String> {
    let want = dunce::canonicalize(folder).ok()?;
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::list_desired_running(&conn).ok()?
    };
    rows.into_iter()
        .find(|r| r.dev_mem_mb.is_some() && dunce::canonicalize(&r.cwd).is_ok_and(|c| c == want))
        .map(|r| r.name)
}

/// How often the idle stop looks: every 30 seconds. The headless test sets
/// `K2_PUBLISH_DEV_IDLE_TICK_MS` (debug builds only).
fn dev_idle_tick() -> Duration {
    const TICK: Duration = Duration::from_secs(30);
    if !cfg!(debug_assertions) {
        return TICK;
    }
    std::env::var("K2_PUBLISH_DEV_IDLE_TICK_MS").ok().and_then(|v| v.parse::<u64>().ok()).map(Duration::from_millis).unwrap_or(TICK)
}

/// The last bind check's exposed addresses, by service id (PV-4): `ps` and
/// `visibility` read this rather than walking `/proc` per request.
fn bind_cache() -> &'static Mutex<HashMap<String, Vec<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What the last bind check saw this service listening on, beyond loopback.
pub(crate) fn exposed_binds(service_id: &str) -> Vec<String> {
    bind_cache().lock().unwrap_or_else(|e| e.into_inner()).get(service_id).cloned().unwrap_or_default()
}

/// Look at what `row`'s program listens on now, and remember it.
fn check_bind(row: &ps::PublishedService) -> Vec<String> {
    let pgid = row.pid.and_then(|p| i32::try_from(p).ok());
    // `None`: K2 couldn't look. Keep the last answer: an unreadable /proc
    // must not read as "nothing is exposed".
    let Some(ls) = crate::publish_dev::listeners(pgid, row.port as u16) else {
        return exposed_binds(&row.id);
    };
    let exposed = crate::publish_dev::exposed_addrs(&ls);
    bind_cache().lock().unwrap_or_else(|e| e.into_inner()).insert(row.id.clone(), exposed.clone());
    exposed
}

/// The check after a start: a moment later, as a server often opens its port
/// before its last listener.
fn bind_check_async(project_id: &str, name: &str) {
    let (project_id, name) = (project_id.to_string(), name.to_string());
    let _ = thread::Builder::new().name(format!("publish-bind-{name}")).spawn(move || {
        thread::sleep(Duration::from_secs(2));
        if let Some(row) = load_row(&project_id, &name) {
            bind_check_one(&row);
        }
    });
}

/// Every 5 minutes (PV-4), for every running `--cmd` service.
fn bind_sweep() {
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::list_desired_running(&conn).unwrap_or_default()
    };
    for row in rows.iter().filter(|r| r.kind != KIND_SKIN && r.cmd != ps::CMD_APP_SENTINEL) {
        bind_check_one(row);
    }
}

fn bind_check_one(row: &ps::PublishedService) {
    let exposed = check_bind(row);
    notify_bind_exposed(row, &exposed);
}

/// Stops an agent scope (C4) when dropped: after the kill sequence above it.
#[cfg(unix)]
struct ScopeStop(Option<(String, String)>);

#[cfg(unix)]
impl Drop for ScopeStop {
    fn drop(&mut self) {
        k2_core::agent_scope::stop_for(self.0.take());
    }
}

fn kill_pid_tree(pid: i32) {
    #[cfg(unix)]
    let _stop_scope = ScopeStop(if pid > 0 { k2_core::agent_scope::scope_of_pid(pid as u32) } else { None });
    #[cfg(unix)]
    unsafe {
        if pid <= 0 {
            return;
        }
        let pgid = libc::getpgid(pid);
        if pgid == pid {
            let _ = libc::killpg(pgid, libc::SIGTERM);
            thread::sleep(KILL_GRACE);
            let pgid2 = libc::getpgid(pid);
            if pgid2 == pid {
                let _ = libc::killpg(pgid2, libc::SIGKILL);
            } else {
                let _ = libc::kill(pid, libc::SIGKILL);
            }
        } else {
            let _ = libc::kill(pid, libc::SIGTERM);
            thread::sleep(KILL_GRACE);
            let _ = libc::kill(pid, libc::SIGKILL);
        }
    }
    // Windows: the process and its descendants (cmd.exe and the service it
    // runs), as killpg on Unix.
    #[cfg(windows)]
    {
        if let Ok(pid) = u32::try_from(pid) {
            k2_core::win_proc::terminate_tree(pid);
        }
    }
}

pub fn rm(project_id: &str, name: &str, keep_hostname: bool) -> Result<(), PublishError> {
    let name = ps::normalize_existing_name(name).map_err(PublishError::bad)?;
    let row = load_row(project_id, &name);
    if let Some(r) = &row {
        refuse_app_row(r)?;
    }
    let _ = stop(project_id, &name);
    // Only ever release THIS workspace's label: a label another workspace
    // holds keeps its attribution and its control-plane hostname.
    let label_is_ours = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let ours = !matches!(
            SubdomainWorkspace::self_unclaim(&conn, &name, project_id),
            Ok(k2_core::db::schema::SelfUnclaim::Foreign { .. })
        );
        let _ = ps::delete(&conn, project_id, &name);
        ours
    };
    // S24-20: logins bound to this App end with it. Its grants stay (shown
    // as "App removed" in Settings) until S26 decides cleanup.
    if let Some(row) = row.as_ref() {
        match k2_core::skin::revoke_sessions_for_app(&row.id) {
            Ok(n) if n > 0 => log_debug!(
                "[publish] rm {project_id}/{name}: ended {n} App login session(s)"
            ),
            Ok(_) => {}
            Err(e) => log_debug!("[publish] rm {project_id}/{name}: WARN end App sessions: {e}"),
        }
        // prd-custom-app-push-v1 CP17: its push devices, settings and keys go.
        crate::app_push::forget_app(&row.id);
        // prd-app-phone-pairing-v1: its pairing config and codes go.
        let runtime = crate::app_pairing::runtime_app(&row.id).map(|a| a.app_id);
        crate::app_pairing::forget_app(&row.id, runtime.as_deref());
        crate::email_notify::forget_app(&row.id);
        // Sew for Apps A6 (Q12): people whose only App this was lose chat
        // (deactivated, never deleted); its guests-chat grant goes.
        if let Err(e) = k2_core::skin::app_chat::app_removed(&row.id, "system:app-removed") {
            log_debug!("[publish] rm {project_id}/{name}: WARN chat users of a removed App: {e}");
        }
    }
    if !keep_hostname && label_is_ours {
        if row.as_ref().is_some_and(|r| r.expose == EXPOSE_TUNNEL) {
            if let Ok(cfg) = tunnel_config::load() {
                if !cfg.token.trim().is_empty() {
                    let _ = subdomains::delete_subdomain(&cfg.token, &name);
                }
            }
        }
    }
    crate::session_events::emit_tunnel_subdomains_changed();
    emit_changed(project_id);
    Ok(())
}

pub fn list(project_id: &str) -> Result<Vec<ServiceJson>, PublishError> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let rows = ps::list_for_project(&conn, project_id).map_err(|e| PublishError::fail(e.to_string()))?;
    Ok(rows.iter().map(to_json).collect())
}

// ── Framing (prd-app-frame-ancestors-v1) ──────────────────────────────

/// Shown on a running `--cmd` App whose own `GET /` is HTML with neither
/// `frame-ancestors` nor `X-Frame-Options` (FA4, FA21). K2 byte-splices
/// those responses and cannot add the header itself.
pub const CMD_FRAME_WARNING: &str = concat!("This App's own server sends no frame-ancestors header on GET /, so any site can put it in a frame. ", product_name!(), " does not change a --cmd App's responses. Add `Content-Security-Policy: frame-ancestors 'self'` in the App (see `", cli_name!(), " study publish-framing`).");

/// The framing headers one loopback `GET`/`HEAD` returned.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FrameProbe {
    pub status: u16,
    pub content_type: String,
    pub csp: Vec<String>,
    pub xfo: Vec<String>,
}

/// One request to `127.0.0.1:<port>`; headers only. `None` when the port
/// does not answer HTTP within ~3 s.
pub fn probe_frame_headers(port: u16, method: &str, path: &str) -> Option<FrameProbe> {
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().ok()?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).ok()?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: text/html\r\nUser-Agent: k2-frame-probe\r\nConnection: close\r\n\r\n"
    );
    s.write_all(req.as_bytes()).ok()?;
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        if raw.windows(4).any(|w| w == b"\r\n\r\n") || raw.len() > 64 * 1024 {
            break;
        }
        match s.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
        }
    }
    let text = String::from_utf8_lossy(&raw);
    let head = text.split("\r\n\r\n").next()?;
    let mut lines = head.split("\r\n");
    let status: u16 = lines.next()?.split_whitespace().nth(1)?.parse().ok()?;
    let mut probe = FrameProbe {
        status,
        ..FrameProbe::default()
    };
    for line in lines {
        let Some((k, v)) = line.split_once(':') else { continue };
        let (k, v) = (k.trim(), v.trim().to_string());
        if k.eq_ignore_ascii_case("content-security-policy") {
            probe.csp.push(v);
        } else if k.eq_ignore_ascii_case("x-frame-options") {
            probe.xfo.push(v);
        } else if k.eq_ignore_ascii_case("content-type") {
            probe.content_type = v;
        }
    }
    Some(probe)
}

/// True when a `--cmd` App's `GET /` is an HTML page anyone may frame.
pub fn cmd_probe_needs_warning(p: &FrameProbe) -> bool {
    (200..300).contains(&p.status)
        && p.content_type.to_ascii_lowercase().starts_with("text/html")
        && !p.csp.iter().any(|v| frame_policy::csp_has_frame_ancestors(v))
        && p.xfo.is_empty()
}

/// True when a helper answers with exactly `policy` (one CSP line, one
/// XFO line, both equal). An older helper build sends neither.
pub fn helper_probe_matches(p: &FrameProbe, policy: &FramePolicy) -> bool {
    let want_xfo: Vec<String> = policy.xfo_value().map(|v| vec![v.to_string()]).unwrap_or_default();
    p.status == 200 && p.csp == vec![policy.csp_value()] && p.xfo == want_xfo
}

type FrameWarnings = HashMap<(String, String), String>;

fn frame_warnings() -> &'static Mutex<FrameWarnings> {
    static W: OnceLock<Mutex<FrameWarnings>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(HashMap::new()))
}

fn frame_warning_get(project_id: &str, name: &str) -> Option<String> {
    frame_warnings()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(project_id.to_string(), name.to_string()))
        .cloned()
}

/// Probe a `--cmd` App and record (or clear) its warning. Emits a change
/// event when the warning flips.
fn probe_cmd_frame(project_id: &str, name: &str, port: u16) {
    let warn = probe_frame_headers(port, "GET", "/")
        .is_some_and(|p| cmd_probe_needs_warning(&p));
    let key = (project_id.to_string(), name.to_string());
    let changed = {
        let mut map = frame_warnings().lock().unwrap_or_else(|e| e.into_inner());
        if warn {
            map.insert(key, CMD_FRAME_WARNING.to_string()).is_none()
        } else {
            map.remove(&key).is_some()
        }
    };
    if changed {
        emit_changed(project_id);
    }
}

fn probe_cmd_frame_async(project_id: &str, name: &str, port: u16) {
    let (p, n) = (project_id.to_string(), name.to_string());
    let _ = thread::Builder::new()
        .name(format!("publish-frame-probe-{name}"))
        .spawn(move || probe_cmd_frame(&p, &n, port));
}

/// Health tick: re-probe every running `--cmd` App.
fn reprobe_cmd_frames() {
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::list_desired_running(&conn).unwrap_or_default()
    };
    for row in rows {
        if row.kind == KIND_SKIN || live_get(&row.project_id, &row.name).is_none() {
            continue;
        }
        probe_cmd_frame(&row.project_id, &row.name, row.port as u16);
    }
}

/// Kill a running `--skin` helper and start it again from its row (new
/// `--frame-ancestors`). The nested hostname is untouched: same port.
/// A stopped App is left stopped.
pub fn restart_skin_helper(project_id: &str, name: &str) -> Result<ServiceJson, PublishError> {
    let row = load_row(project_id, name)
        .ok_or_else(|| PublishError::bad("no such published service"))?;
    if row.kind != KIND_SKIN {
        return Err(PublishError::bad("not a --skin App"));
    }
    let entry = live_remove(project_id, name);
    let had_live = entry.is_some();
    if let Some(entry) = entry {
        kill_tree(&entry);
    } else if let Some(pid) = row.pid.and_then(|p| i32::try_from(p).ok()) {
        if pid_alive(pid) {
            kill_pid_tree(pid);
        }
    }
    if !had_live && row.desired != DESIRED_RUNNING {
        return Ok(to_json(&row));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while port_accepts(row.port as u16) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
    if port_accepts(row.port as u16) {
        return Err(PublishError::fail(format!(
            "restart: 127.0.0.1:{} is still in use after stopping the helper",
            row.port
        )));
    }
    start_inner(project_id, name, false, false)
}

/// Boot reattach of a `--skin` helper: if it does not answer with the
/// policy its row asks for (an older build sends no framing headers at
/// all), restart it so the upgrade takes effect without an owner action.
fn restart_helper_if_frame_drift(row: &ps::PublishedService) {
    let Ok(want) = FramePolicy::from_stored(&row.frame_ancestors) else {
        log_debug!(
            "[publish] {}/{} has an invalid frame policy; leaving it",
            row.project_id,
            row.name
        );
        return;
    };
    let got = probe_frame_headers(row.port as u16, "HEAD", "/login");
    if got.as_ref().is_some_and(|p| helper_probe_matches(p, &want)) {
        return;
    }
    log_debug!(
        "[publish] {}/{} helper frame policy drift ({:?}); restarting",
        row.project_id,
        row.name,
        got
    );
    if let Err(e) = restart_skin_helper(&row.project_id, &row.name) {
        log_debug!(
            "[publish] {}/{} drift restart failed: {}",
            row.project_id,
            row.name,
            e.message
        );
    }
}

/// What `POST /cli/publish/frame` asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameChange {
    Allow(Vec<String>),
    None,
    Reset,
}

/// `k2 publish expose <name> public|server` (0.46.18), the owner's: a
/// `--cmd` service that listens beyond this server on purpose (no warning, no
/// loopback variables at its next start), or back to this server only. `by`
/// names who, for the record and the access audit log. The running process
/// is not touched: its environment changes at its next start.
pub fn set_expose(project_id: &str, name: &str, public: bool, by: &str) -> Result<ServiceJson, PublishError> {
    let name = ps::normalize_existing_name(name).map_err(PublishError::bad)?;
    let row = load_row(project_id, &name).ok_or_else(|| PublishError::bad("no such published service"))?;
    if row.kind == KIND_SKIN || row.cmd == ps::CMD_APP_SENTINEL {
        return Err(PublishError::bad(format!(
            "expose is for --cmd services. An App and a --skin site listen where {} starts them, on this server only",
            k2_core::brand::PRODUCT_NAME
        )));
    }
    crate::publish_expose::set(&row.id, public, by).map_err(PublishError::fail)?;
    let value = if public { crate::publish_expose::PUBLIC } else { crate::publish_expose::SERVER };
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "publish-expose",
        by,
        format!("ok service={name} id={} expose={value}", row.id),
        "-",
        "-",
        "cli",
    ));
    if !public {
        // Back to this server only: warn again at once, as for new addresses.
        bind_noticed().lock().unwrap_or_else(|e| e.into_inner()).retain(|(id, _)| id != &row.id);
    }
    emit_changed(project_id);
    Ok(to_json(&row))
}

/// Owner: change a `--skin` App's framing policy, then restart its helper
/// when it is running. Returns the service JSON and whether it restarted.
pub fn set_frame(
    project_id: &str,
    name: &str,
    change: FrameChange,
) -> Result<(ServiceJson, bool), FrameSetError> {
    let name = ps::normalize_existing_name(name).map_err(|e| FrameSetError::Publish(PublishError::bad(e)))?;
    let row = load_row(project_id, &name).ok_or_else(|| {
        FrameSetError::Publish(PublishError::bad("no such published service"))
    })?;
    if row.kind != KIND_SKIN {
        return Err(FrameSetError::Publish(PublishError::bad(
            concat!("frame policy is for --skin Apps. A --cmd App sets its own header (", cli_name!(), " study publish-framing)"),
        )));
    }
    let stored = match change {
        FrameChange::Reset => frame_policy::STORED_DEFAULT.to_string(),
        FrameChange::None => frame_policy::STORED_NONE.to_string(),
        FrameChange::Allow(adds) => {
            frame_policy::stored_with_allow(&row.frame_ancestors, &adds).map_err(FrameSetError::Policy)?
        }
    };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::set_frame_ancestors(&conn, project_id, &name, &stored)
            .map_err(|e| FrameSetError::Publish(PublishError::fail(e.to_string())))?;
    }
    let running = is_running_now(project_id, &name, &row);
    let json = if running {
        restart_skin_helper(project_id, &name).map_err(FrameSetError::Publish)?
    } else {
        let row = load_row(project_id, &name).unwrap_or(row);
        to_json(&row)
    };
    emit_changed(project_id);
    Ok((json, running))
}

#[derive(Debug)]
pub enum FrameSetError {
    Publish(PublishError),
    Policy(frame_policy::FrameError),
}

/// Boot: desired=running → reattach if pid alive AND port accepts, else respawn.
/// Stagger between services. Bind clash fails loud (error on the row).
pub fn boot_desired_running() {
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::list_desired_running(&conn).unwrap_or_default()
    };
    // App runtime rows (AR5) belong to the App supervisor.
    let rows: Vec<_> = rows.into_iter().filter(|r| r.cmd != ps::CMD_APP_SENTINEL).collect();
    for (i, row) in rows.into_iter().enumerate() {
        if i > 0 {
            thread::sleep(stagger());
        }
        let port = row.port as u16;
        let pid = row.pid.and_then(|p| i32::try_from(p).ok()).unwrap_or(0);
        if pid > 0 && pid_alive(pid) && port_accepts(port) {
            log_debug!(
                "[publish] boot reattach {}/{} pid={}",
                row.project_id,
                row.name,
                pid
            );
            let entry = adopt_reattach(&row.project_id, &row.name, port, pid);
            emit_changed(&row.project_id);
            if row.hostname_pending && row.expose == EXPOSE_TUNNEL {
                // The daemon went down inside this service's grace and the
                // port has answered since: publish the hostname now.
                let r = row.clone();
                let _ = thread::Builder::new()
                    .name(format!("publish-attach-{}", row.name))
                    .spawn(move || {
                        if let Err(e) = finish_start(&r.project_id, &r.name, &entry, &r, true, false) {
                            log_debug!(
                                "[publish] boot hostname attach {}/{} failed: {}",
                                r.project_id,
                                r.name,
                                e.message
                            );
                        }
                    });
            }
            if row.kind == KIND_SKIN {
                // S24-8: a helper from before per-App logins (macOS keeps
                // helpers across a daemon update) logs guests in as `host`.
                // Restart it so the App's own grants apply (Q4).
                if !restart_helper_if_unbound(&row, pid) {
                    restart_helper_if_frame_drift(&row);
                }
            } else {
                probe_cmd_frame_async(&row.project_id, &row.name, port);
            }
            continue;
        }
        if port_accepts(port) && !(pid > 0 && pid_alive(pid)) {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = ps::set_error(
                &conn,
                &row.project_id,
                &row.name,
                Some(&format!(
                    "bind clash on boot: 127.0.0.1:{port} is already in use"
                )),
            );
            emit_changed(&row.project_id);
            continue;
        }
        if pid > 0 && pid_alive(pid) {
            kill_pid_tree(pid);
            thread::sleep(Duration::from_millis(100));
        }
        log_debug!(
            "[publish] boot respawn {}/{}",
            row.project_id,
            row.name
        );
        if let Err(e) = start_inner(&row.project_id, &row.name, row.expose == EXPOSE_TUNNEL, false) {
            log_debug!(
                "[publish] boot respawn {}/{} failed: {}",
                row.project_id,
                row.name,
                e.message
            );
        }
    }
}

/// Q4 default (S24-8): at boot, restart a reattached `--skin` helper whose
/// argv lacks `--app <its service id>` (an older binary's helper still
/// logging guests in as `host`). Those guests sign in once more; per-App
/// enforcement is immediate. `false` = leave it running until its next
/// restart (Settings shows `helperNeedsRestart`).
pub const RESTART_UNBOUND_HELPERS_AT_BOOT: bool = true;

/// `--skin` helpers seen at boot without `--app` that were left running
/// (or failed to restart). Read by [`to_json`] for the Settings badge.
fn unbound_helpers() -> &'static parking_lot::Mutex<std::collections::HashSet<String>> {
    static SET: std::sync::OnceLock<parking_lot::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    SET.get_or_init(|| parking_lot::Mutex::new(std::collections::HashSet::new()))
}

/// The argv of a live process, or `None` when it can't be read. Linux
/// reads `/proc/<pid>/cmdline`; macOS asks the kernel. Not for a child spawned a
/// moment ago: on Linux its cmdline reads empty for a few ms after `spawn`
/// returns (the kernel sets the argv bounds after releasing the parent).
/// The one caller reads helpers reattached from an earlier daemon life.
fn process_argv(pid: i32) -> Option<Vec<String>> {
    #[cfg(target_os = "linux")]
    {
        let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        let parts: Vec<String> = raw
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts)
        }
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        k2_core::mac_proc::argv_or_ps(pid)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

/// True when this argv binds the helper to `service_id` (`--app ID` or
/// `--app=ID`).
pub fn argv_binds_app(argv: &[String], service_id: &str) -> bool {
    argv.iter().enumerate().any(|(i, a)| {
        a.strip_prefix("--app=") == Some(service_id)
            || (a == "--app" && argv.get(i + 1).map(String::as_str) == Some(service_id))
    })
}

/// S24-8 boot check for one reattached `--skin` helper. Returns true when it
/// restarted the helper (the caller then skips the frame check: a fresh
/// helper has the row's policy). Unreadable argv (Windows): left alone and
/// logged, since a restart can't be shown to be needed.
fn restart_helper_if_unbound(row: &ps::PublishedService, pid: i32) -> bool {
    let Some(argv) = process_argv(pid) else {
        log_debug!(
            "[publish] {}/{} could not read helper argv (pid {pid}); per-App login binding unchecked",
            row.project_id,
            row.name
        );
        return false;
    };
    if argv_binds_app(&argv, &row.id) {
        unbound_helpers().lock().remove(&row.id);
        return false;
    }
    if !RESTART_UNBOUND_HELPERS_AT_BOOT {
        log_debug!(
            "[publish] {}/{} helper predates per-App logins; left running (signs guests in as host)",
            row.project_id,
            row.name
        );
        unbound_helpers().lock().insert(row.id.clone());
        return false;
    }
    log_debug!(
        "[publish] {}/{} helper predates per-App logins; restarting it with --app",
        row.project_id,
        row.name
    );
    match restart_skin_helper(&row.project_id, &row.name) {
        Ok(_) => {
            unbound_helpers().lock().remove(&row.id);
            true
        }
        Err(e) => {
            log_debug!(
                "[publish] {}/{} per-App restart failed: {}",
                row.project_id,
                row.name,
                e.message
            );
            unbound_helpers().lock().insert(row.id.clone());
            false
        }
    }
}

/// True while this App's helper is an older one still signing guests in as
/// `host` (Settings badge, S24-15).
pub fn helper_needs_restart(service_id: &str) -> bool {
    unbound_helpers().lock().contains(service_id)
}

/// S24-10: once per skin.db, before any helper starts, copy every box login
/// onto each `--skin` App that exists now, so nobody who can sign in to an
/// App today is locked out by per-App logins. Failure is loud and retried
/// at the next boot (the pending mark stays).
pub fn run_skin_app_backfill_at_boot() {
    match k2_core::skin::skin_app_backfill_pending() {
        Ok(false) => return,
        Ok(true) => {}
        Err(e) => {
            log_debug!("[publish] ERROR per-App login backfill: skin.db: {e}");
            return;
        }
    }
    let ids = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::ids_of_kind(&conn, KIND_SKIN)
    };
    let ids = match ids {
        Ok(ids) => ids,
        Err(e) => {
            log_debug!("[publish] ERROR per-App login backfill: published services: {e}");
            return;
        }
    };
    match k2_core::skin::run_skin_app_backfill(&ids) {
        Ok(Some(reports)) => log_debug!(
            "[publish] per-App login backfill: {} --skin App(s), {} login(s) copied",
            reports.len(),
            reports.iter().map(|r| r.added.len()).sum::<usize>()
        ),
        Ok(None) => {}
        Err(e) => log_debug!("[publish] ERROR per-App login backfill (retried next boot): {e}"),
    }
}

/// Long-lived supervisor thread: boot respawn then idle (child waiters
/// are per-service threads).
pub fn spawn() -> thread::JoinHandle<()> {
    // DM-6: the gate's way to the dev process lane, before anything is served.
    crate::site_gate::dev::install(crate::site_gate::dev::DevHooks { touch: dev_touch, wake: dev_wake });
    thread::Builder::new()
        .name("publish-runtime".into())
        .spawn(|| {
            run_skin_app_backfill_at_boot();
            boot_desired_running();
            // DM-6: the idle stop has its own thread, so a slow frame or bind
            // check never holds it up.
            let _ = thread::Builder::new().name("publish-dev-idle".into()).spawn(|| loop {
                thread::sleep(dev_idle_tick());
                dev_idle_sweep(Instant::now());
            });
            let mut ticks: u32 = 0;
            loop {
                thread::sleep(Duration::from_secs(30));
                ticks = ticks.wrapping_add(1);
                // `--cmd` frame check on the health tick (FA4): every 10 min.
                if ticks % 20 == 0 {
                    reprobe_cmd_frames();
                }
                // PV-4: what each service listens on, every 5 min.
                if ticks % 10 == 0 {
                    bind_sweep();
                }
                upgrade_card_tick();
            }
        })
        .expect("spawn publish-runtime")
}

#[cfg(windows)]
pub(crate) mod win_job {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command};

    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_SUSPENDED: u32 = 0x0000_0004;
    const TH32CS_SNAPTHREAD: u32 = 0x0000_0004;
    const THREAD_SUSPEND_RESUME: u32 = 0x0002;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;

    #[repr(C)]
    struct JobBasicLimit {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    struct JobExtendedLimit {
        basic: JobBasicLimit,
        io: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    extern "system" {
        fn CreateJobObjectW(a: *mut core::ffi::c_void, name: *const u16) -> *mut core::ffi::c_void;
        fn SetInformationJobObject(
            job: *mut core::ffi::c_void,
            class: i32,
            info: *mut core::ffi::c_void,
            len: u32,
        ) -> i32;
        fn AssignProcessToJobObject(
            job: *mut core::ffi::c_void,
            process: *mut core::ffi::c_void,
        ) -> i32;
        fn TerminateJobObject(job: *mut core::ffi::c_void, code: u32) -> i32;
        fn CloseHandle(h: *mut core::ffi::c_void) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut core::ffi::c_void;
        fn GetExitCodeProcess(h: *mut core::ffi::c_void, code: *mut u32) -> i32;
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut core::ffi::c_void;
        fn Thread32First(snap: *mut core::ffi::c_void, te: *mut ThreadEntry) -> i32;
        fn Thread32Next(snap: *mut core::ffi::c_void, te: *mut ThreadEntry) -> i32;
        fn OpenThread(access: u32, inherit: i32, tid: u32) -> *mut core::ffi::c_void;
        fn ResumeThread(h: *mut core::ffi::c_void) -> u32;
    }

    /// THREADENTRY32.
    #[repr(C)]
    struct ThreadEntry {
        size: u32,
        usage: u32,
        thread_id: u32,
        owner_pid: u32,
        base_pri: i32,
        delta_pri: i32,
        flags: u32,
    }

    /// Resume every thread of `pid` (a CREATE_SUSPENDED child has one).
    /// `false` when none could be resumed.
    fn resume_process(pid: u32) -> bool {
        const INVALID: isize = -1;
        let mut resumed = false;
        // SAFETY: a snapshot handle checked and closed once; the entry is
        // sized as the API requires; thread handles are closed after use.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap.is_null() || snap as isize == INVALID {
                return false;
            }
            let mut te: ThreadEntry = std::mem::zeroed();
            te.size = std::mem::size_of::<ThreadEntry>() as u32;
            let mut ok = Thread32First(snap, &mut te) != 0;
            while ok {
                if te.owner_pid == pid {
                    let th = OpenThread(THREAD_SUSPEND_RESUME, 0, te.thread_id);
                    if !th.is_null() {
                        resumed |= ResumeThread(th) != u32::MAX;
                        CloseHandle(th);
                    }
                }
                ok = Thread32Next(snap, &mut te) != 0;
            }
            CloseHandle(snap);
        }
        resumed
    }

    pub struct Job(*mut core::ffi::c_void);

    unsafe impl Send for Job {}

    impl Drop for Job {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    CloseHandle(self.0);
                }
                self.0 = std::ptr::null_mut();
            }
        }
    }

    impl Job {
        pub fn terminate(&self) {
            if !self.0.is_null() {
                unsafe {
                    TerminateJobObject(self.0, 1);
                }
            }
        }

        /// The pids of the processes in this job right now (an empty list
        /// once every one has exited).
        pub fn pids(&self) -> Vec<u32> {
            if self.0.is_null() {
                return Vec::new();
            }
            let mut list: Box<wait::PidList> = Box::new(unsafe { std::mem::zeroed() });
            let mut ret = 0u32;
            // SAFETY: a live job handle; the buffer is the size we pass. A
            // full buffer still fills `listed` entries (ERROR_MORE_DATA).
            unsafe {
                wait::QueryInformationJobObject(
                    self.0,
                    wait::JOB_OBJECT_BASIC_PROCESS_ID_LIST,
                    (&mut *list as *mut wait::PidList).cast(),
                    std::mem::size_of::<wait::PidList>() as u32,
                    &mut ret,
                );
            }
            let n = (list.listed as usize).min(list.ids.len());
            list.ids[..n].iter().map(|p| *p as u32).collect()
        }

        /// Terminate every process in the job and wait, up to `cap`, until
        /// each has fully exited (a signalled process has closed its
        /// handles, sockets included) and the job lists none. `Err` holds
        /// the pids still running at the cap.
        pub fn terminate_and_wait(&self, cap: std::time::Duration) -> Result<(), Vec<u32>> {
            let deadline = std::time::Instant::now() + cap;
            // Handles first, so a pid can't be reused under the wait.
            let handles: Vec<(u32, *mut core::ffi::c_void)> = self
                .pids()
                .into_iter()
                .filter_map(|pid| {
                    let h = unsafe { OpenProcess(wait::SYNCHRONIZE, 0, pid) };
                    (!h.is_null()).then_some((pid, h))
                })
                .collect();
            self.terminate();
            let mut left = Vec::new();
            for (pid, h) in handles {
                let ms = deadline.saturating_duration_since(std::time::Instant::now()).as_millis();
                // SAFETY: a process handle we opened, closed once here.
                let r = unsafe { wait::WaitForSingleObject(h, ms.min(u32::MAX as u128 - 1) as u32) };
                unsafe { CloseHandle(h) };
                if r != wait::WAIT_OBJECT_0 {
                    left.push(pid);
                }
            }
            // The job's own list (a process that joined after the first
            // read, or one whose handle couldn't be opened).
            loop {
                let still = self.pids();
                if still.is_empty() && left.is_empty() {
                    return Ok(());
                }
                if std::time::Instant::now() >= deadline {
                    for p in still {
                        if !left.contains(&p) {
                            left.push(p);
                        }
                    }
                    return Err(left);
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }

    /// Job membership and process waits, for a stop that waits.
    mod wait {
        pub const JOB_OBJECT_BASIC_PROCESS_ID_LIST: i32 = 3;
        pub const SYNCHRONIZE: u32 = 0x0010_0000;
        pub const WAIT_OBJECT_0: u32 = 0;

        /// JOBOBJECT_BASIC_PROCESS_ID_LIST, room for 1024 pids.
        #[repr(C)]
        pub struct PidList {
            pub assigned: u32,
            pub listed: u32,
            pub ids: [usize; 1024],
        }

        extern "system" {
            pub fn QueryInformationJobObject(
                job: *mut core::ffi::c_void,
                class: i32,
                info: *mut core::ffi::c_void,
                len: u32,
                ret: *mut u32,
            ) -> i32;
            pub fn WaitForSingleObject(h: *mut core::ffi::c_void, ms: u32) -> u32;
        }
    }

    /// Start `cmd` inside a new kill-on-close job. The child is created
    /// suspended and resumed only once it is in the job: started running,
    /// it could start its own children (cmd.exe running the service)
    /// before the assignment, and those would escape the job and outlive
    /// a stop.
    pub fn spawn_in_job(cmd: &mut Command) -> Result<(Child, Job), String> {
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW | CREATE_SUSPENDED);
        let child = cmd.spawn().map_err(|e| format!("spawn: {e}"))?;
        let job_h = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
        if job_h.is_null() {
            let mut child = child;
            let _ = child.kill();
            return Err(
                "CreateJobObjectW failed — Job Objects are required for published services on Windows"
                    .into(),
            );
        }
        let mut info: JobExtendedLimit = unsafe { std::mem::zeroed() };
        info.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = unsafe {
            SetInformationJobObject(
                job_h,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                &mut info as *mut _ as *mut core::ffi::c_void,
                std::mem::size_of::<JobExtendedLimit>() as u32,
            )
        };
        if ok == 0 {
            let mut child = child;
            let _ = child.kill();
            unsafe { CloseHandle(job_h) };
            return Err("SetInformationJobObject failed — Job Objects are required".into());
        }
        let proc = child.as_raw_handle() as *mut core::ffi::c_void;
        if unsafe { AssignProcessToJobObject(job_h, proc) } == 0 {
            let mut child = child;
            let _ = child.kill();
            unsafe { CloseHandle(job_h) };
            return Err("AssignProcessToJobObject failed — Job Objects are required".into());
        }
        if !resume_process(child.id()) {
            let mut child = child;
            let _ = child.kill();
            unsafe { CloseHandle(job_h) };
            return Err("could not resume the new process after placing it in its job".into());
        }
        Ok((child, Job(job_h)))
    }

    pub fn pid_alive(pid: u32) -> bool {
        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        const STILL_ACTIVE: u32 = 259;
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return false;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(h, &mut code);
            CloseHandle(h);
            ok != 0 && code == STILL_ACTIVE
        }
    }
}

/// Kill every published child THIS process spawned (skin-gateway helpers,
/// `--cmd` services) and forget them — no DB writes, no events. Test
/// harness teardown: the helpers are `setsid` session leaders, so a test
/// binary that exits (or panics past its `stop`) would otherwise leave
/// `k2-daemon --skin-gateway` orphans listening forever (two from the
/// 2026-09-12 run were killed by hand). Production never calls this —
/// helpers are meant to outlive a daemon restart (`boot_desired_running`
/// reattaches by pid + port). That holds on macOS (launchd kills only the
/// daemon) and, since runtime stage H, on Linux where the unit carries the
/// root helper's `k2-handoff.conf` drop-in (`KillMode=process`; before it,
/// `KillMode=control-group` killed every helper with the daemon — AR3) and
/// the helpers log straight into their file (a pumped pipe died with the
/// daemon and took the helper with it on its next write).
#[allow(dead_code)] // lib-only caller (integration harness teardown); the bin target never calls it
pub fn kill_all_live_children() {
    let entries: Vec<Arc<LiveChild>> = {
        let mut map = live().lock().unwrap_or_else(|e| e.into_inner());
        map.drain().map(|(_, v)| v).collect()
    };
    for entry in entries {
        kill_tree(&entry);
    }
}

#[cfg(test)]
#[allow(dead_code)]
pub fn kill_all_for_tests() {
    let keys: Vec<(String, String)> = live()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .keys()
        .cloned()
        .collect();
    for (pid, name) in keys {
        let _ = stop(&pid, &name);
    }
    kill_all_live_children();
}

#[cfg(test)]
#[path = "publish_runtime_tests.rs"]
mod tests;
