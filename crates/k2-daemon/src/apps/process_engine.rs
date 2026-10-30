//! The stage-1 **process** engine (§13 S7, AR34): today's publish
//! supervisor promoted behind `RuntimeEngine`.
//!
//! - Spawn the Seed's `host.start` argv (no shell) in the Seed folder, in
//!   its own process group (`setsid`), as the daemon user. That is the
//!   recorded exception to SR1 (§16 C9). Since 0.46.3 it is a separate
//!   owner opt-in (`K2_APPS_PROCESS=1` / the `appsProcessEngine` setting,
//!   default off; `deploy::process_engine_enabled`): the default-on Apps
//!   switch never selects it. The guarded container engine is the default.
//! - **Clean environment** (S1 applied): `env_clear()`, then `PATH`
//!   (login-shell enriched), `LANG`, `LC_*`, `TZ`, `TMPDIR`, `HOME` (the App
//!   folder's `home/`), `PORT`, `NODE_ENV`, the App identity (S24's
//!   `K2_APP_ID` = the published service UUID, `K2_HOST`, `K2_PORT`; plus
//!   `K2_APP_NAME` = the App's slug, AR37), `K2_APP_DEPLOYMENT`,
//!   `K2_APP_DATA`, `K2_APP_CACHE`, the manifest's declared variables, then
//!   the vault's grants for the App's service id.
//!   Never the daemon's `K2_*` (hook socket, scoped token, API flags).
//! - Per-deployment log (`~/.k2/apps/<id>/logs/<deployment>.log`), rotated
//!   once at 50 MiB. Since 0.46.3 (smoke r6) the App's stdout and stderr
//!   ARE that file (an `O_APPEND` descriptor the App holds), not a pipe to
//!   the daemon: an App that outlives a daemon restart (`KillMode=process`)
//!   keeps logging and answering. Before, its pipe pointed at the dead
//!   daemon, every write failed, requests got empty replies while `k2 app
//!   list` said running, and the health checks restarted it minutes later
//!   as a crash. Rotation is copy-and-truncate (the descriptor stays put).
//!   `K2_APPS_PROCESS_LOG=pipe` puts back the old pipe (the way back while
//!   the file sink proves itself).
//! - `adopt`: each instance writes `~/.k2/apps/<id>/run/<deployment>.json`
//!   (pid, port, log sink); after a daemon restart a live pid whose process
//!   group is its own is re-adopted. One that logged through a pipe (an
//!   instance an older daemon started, or the `pipe` opt-out) can't be
//!   re-attached: [`needs_relaunch`] says so, and the supervisor starts a
//!   fresh instance at once (not a crash, no agent woken).
//! - No memory limit is enforced here (`caps().memory_limit = false`); a
//!   stored per-App limit takes effect with the walled engines (stage 3/R).

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

use k2_core::deploy::engine::{
    AppSpec, CgroupStats, EngineCaps, EngineError, ExecStream, Isolation, Liveness, Prepared, Result, RunHandle, RuntimeEngine,
    SeedRef,
};
use k2_core::deploy::{self, codes};
use serde::{Deserialize, Serialize};

pub struct ProcessEngine;

#[derive(Debug, Serialize, Deserialize)]
struct RunFile {
    app_id: String,
    deployment_id: String,
    port: u16,
    pid: i32,
    /// Where the App's stdout/stderr go: `file` (its log, held by the App)
    /// or `pipe` (to the daemon that started it). Run files from before
    /// 0.46.3 have none: a pipe.
    #[serde(default = "pipe_sink")]
    log: String,
}

const SINK_FILE: &str = "file";
const SINK_PIPE: &str = "pipe";

fn pipe_sink() -> String {
    SINK_PIPE.to_string()
}

/// The way back (`K2_APPS_PROCESS_LOG=pipe`): the pre-0.46.3 pipe and pump.
fn pipe_sink_wanted() -> bool {
    std::env::var("K2_APPS_PROCESS_LOG").is_ok_and(|v| v.trim().eq_ignore_ascii_case(SINK_PIPE))
}

/// Deployments adopted at boot whose stdout/stderr were a pipe to the old
/// daemon (nothing reads it any more): they need a fresh instance.
fn stale_sinks() -> &'static Mutex<std::collections::HashSet<String>> {
    static S: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

/// Was this adopted instance logging through a pipe to the daemon that
/// started it (so its writes fail now)? The supervisor relaunches it.
pub fn needs_relaunch(deployment_id: &str) -> bool {
    stale_sinks().lock().unwrap_or_else(|e| e.into_inner()).contains(deployment_id)
}

/// Logs held by Apps (the file sink): rotated by copy-and-truncate.
fn file_logs() -> &'static Mutex<HashMap<String, PathBuf>> {
    static L: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Copy-and-truncate a log an App holds open with `O_APPEND`: the copy
/// becomes `<log>.1`, the App's next write lands at the start of the
/// emptied file. (A rename would leave the App writing to `.1`.)
pub(crate) fn rotate_held_log(path: &Path, cap: u64) -> bool {
    let Ok(meta) = fs::metadata(path) else { return false };
    if meta.len() <= cap {
        return false;
    }
    let bak = path.with_extension("log.1");
    if fs::copy(path, &bak).is_err() {
        return false;
    }
    OpenOptions::new().write(true).open(path).and_then(|f| f.set_len(0)).is_ok()
}

fn watch_file_log(deployment_id: &str, path: &Path) {
    file_logs().lock().unwrap_or_else(|e| e.into_inner()).insert(deployment_id.to_string(), path.to_path_buf());
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    let _ = std::thread::Builder::new().name("app-log-rotate".into()).spawn(|| loop {
        std::thread::sleep(Duration::from_secs(30));
        let paths: Vec<PathBuf> = file_logs().lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect();
        for p in paths {
            rotate_held_log(&p, deploy::LOG_CAP_BYTES);
        }
    });
}

struct Live {
    child: Option<Child>,
    secrets_dir: Option<PathBuf>,
}

fn children() -> &'static Mutex<HashMap<String, Live>> {
    static C: OnceLock<Mutex<HashMap<String, Live>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Windows: each App's Job Object (kill-on-close), by deployment. Stop
/// ends the whole tree through it; the setsid process group's role.
#[cfg(windows)]
fn jobs() -> &'static Mutex<HashMap<String, crate::publish_runtime::win_job::Job>> {
    static J: OnceLock<Mutex<HashMap<String, crate::publish_runtime::win_job::Job>>> = OnceLock::new();
    J.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Windows programs need the system's own variables to start (without
/// SystemRoot, node can't load its crypto), and look for their home and
/// temp folder under these names.
#[cfg(windows)]
fn windows_base_env(spec: &AppSpec) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for k in ["SystemRoot", "SystemDrive", "windir", "ComSpec", "PATHEXT", "APPDATA", "LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)", "ProgramData", "NUMBER_OF_PROCESSORS", "PROCESSOR_ARCHITECTURE"] {
        if let Ok(v) = std::env::var(k) {
            out.push((k.to_string(), v));
        }
    }
    let tmp = spec.cache_dir.join("tmp").to_string_lossy().to_string();
    out.push(("TEMP".into(), tmp.clone()));
    out.push(("TMP".into(), tmp));
    out.push(("USERPROFILE".into(), spec.home_dir.to_string_lossy().to_string()));
    out
}

fn run_dir(app_id: &str) -> PathBuf {
    deploy::app_dir(app_id).join("run")
}

fn run_file(app_id: &str, deployment_id: &str) -> PathBuf {
    run_dir(app_id).join(format!("{deployment_id}.json"))
}

/// Variables K2 sets itself; a manifest can't declare these.
const OWN: &[&str] = &["PATH", "HOME", "PORT", "NODE_ENV", "TMPDIR", "LANG", "TZ"];

/// The App's slug (its CLI/URL handle). `K2_APP_ID` keeps S24's meaning,
/// the published service UUID, which existing BFFs send on login (AR37).
pub const APP_NAME_ENV: &str = "K2_APP_NAME";

/// The clean environment (S1 applied; §4.2 item 9's allowlist).
pub fn clean_env(spec: &AppSpec, port: u16) -> Vec<(String, String)> {
    clean_env_with_daemon_port(spec, port, crate::publish_runtime::loopback_port())
}

fn clean_env_with_daemon_port(spec: &AppSpec, port: u16, daemon_port: u16) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let inherited = std::env::var("PATH").unwrap_or_default();
    out.push(("PATH".into(), k2_core::terminal::login_path::augmented_path(&inherited)));
    for (k, v) in std::env::vars() {
        if k == "LANG" || k == "TZ" || k.starts_with("LC_") {
            out.push((k, v));
        }
    }
    out.push(("TMPDIR".into(), spec.cache_dir.join("tmp").to_string_lossy().to_string()));
    out.push(("HOME".into(), spec.home_dir.to_string_lossy().to_string()));
    out.push(("PORT".into(), port.to_string()));
    out.push(("NODE_ENV".into(), "production".into()));
    for (k, v) in &spec.env {
        if !OWN.contains(&k.as_str()) && !k.starts_with("K2_") {
            out.push((k.clone(), v.clone()));
        }
    }
    // Same names and meanings as every published `--cmd` service (S24-9):
    // K2_APP_ID = service UUID, K2_HOST/K2_PORT = the daemon's loopback.
    for (k, v) in crate::vault::publish_env::app_identity_env(&spec.service_id, daemon_port) {
        out.push((k.into(), v));
    }
    out.push((APP_NAME_ENV.into(), spec.app_id.clone()));
    out.push(("K2_APP_DEPLOYMENT".into(), spec.deployment_id.clone()));
    out.push(("K2_APP_DATA".into(), spec.data_dir.to_string_lossy().to_string()));
    out.push(("K2_APP_CACHE".into(), spec.cache_dir.to_string_lossy().to_string()));
    for (k, v) in super::origins::env_pairs(&spec.public_origins) {
        out.push((k.into(), v));
    }
    out
}

fn rotate_if_needed(path: &Path) {
    let Ok(meta) = fs::metadata(path) else { return };
    if meta.len() <= deploy::LOG_CAP_BYTES {
        return;
    }
    let bak = path.with_extension("log.1");
    let _ = fs::remove_file(&bak);
    let _ = fs::rename(path, &bak);
}

fn pump(mut src: impl Read + Send + 'static, path: PathBuf) {
    let _ = std::thread::Builder::new().name("app-log".into()).spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            let n = match src.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            rotate_if_needed(&path);
            if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
                let _ = f.write_all(&buf[..n]);
            }
        }
    });
}

#[cfg(unix)]
fn pid_alive(pid: i32) -> bool {
    pid > 0 && unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(unix)]
fn own_group(pid: i32) -> bool {
    pid > 0 && unsafe { libc::getpgid(pid) == pid }
}

#[cfg(unix)]
fn signal_group(pid: i32, sig: i32) {
    if pid <= 0 {
        return;
    }
    unsafe {
        if own_group(pid) {
            let _ = libc::killpg(pid, sig);
        } else {
            let _ = libc::kill(pid, sig);
        }
    }
}

/// The signal that ended one of our own children (`None`: still running,
/// not ours, exited with a code, or not Unix). For the crash record; the
/// exit code comes from `liveness`.
pub fn exit_signal(deployment_id: &str) -> Option<i32> {
    let mut c = children().lock().unwrap_or_else(|e| e.into_inner());
    let st = c.get_mut(deployment_id)?.child.as_mut()?.try_wait().ok()??;
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        st.signal()
    }
    #[cfg(not(unix))]
    {
        let _ = st;
        None
    }
}

impl ProcessEngine {
    /// Reap our own child when it exited; `Some(code)` once it has.
    fn reap(&self, deployment_id: &str) -> Option<Option<i32>> {
        let mut c = children().lock().unwrap_or_else(|e| e.into_inner());
        let live = c.get_mut(deployment_id)?;
        let child = live.child.as_mut()?;
        match child.try_wait() {
            Ok(Some(st)) => Some(st.code()),
            Ok(None) => None,
            Err(_) => Some(None),
        }
    }

    /// The instance is gone (our child reaped, or an adopted pid dead).
    fn exited(&self, h: &RunHandle) -> bool {
        if self.reap(&h.deployment_id).is_some() {
            return true;
        }
        let owned = children()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&h.deployment_id)
            .is_some_and(|l| l.child.is_some());
        #[cfg(unix)]
        {
            !owned && !h.pid.is_some_and(pid_alive)
        }
        // Windows never adopts: an instance this daemon didn't start (no
        // child) is gone, whatever process now holds its old pid.
        #[cfg(windows)]
        {
            !owned
        }
        #[cfg(not(any(unix, windows)))]
        {
            !owned
        }
    }

    fn forget(&self, h: &RunHandle) {
        file_logs().lock().unwrap_or_else(|e| e.into_inner()).remove(&h.deployment_id);
        stale_sinks().lock().unwrap_or_else(|e| e.into_inner()).remove(&h.deployment_id);
        // Windows: dropping the job (kill-on-close) ends anything left.
        #[cfg(windows)]
        jobs().lock().unwrap_or_else(|e| e.into_inner()).remove(&h.deployment_id);
        let removed = children().lock().unwrap_or_else(|e| e.into_inner()).remove(&h.deployment_id);
        if let Some(Live { secrets_dir: Some(d), .. }) = removed {
            let _ = fs::remove_dir_all(d);
        }
        let _ = fs::remove_file(run_file(&h.app_id, &h.deployment_id));
    }
}

impl RuntimeEngine for ProcessEngine {
    fn isolation(&self) -> Isolation {
        Isolation::Process
    }

    fn caps(&self) -> EngineCaps {
        EngineCaps { adopt: cfg!(unix), ..EngineCaps::default() }
    }

    fn prepare(&self, seed: &SeedRef, spec: &AppSpec) -> Result<Prepared> {
        for d in [&spec.data_dir, &spec.cache_dir, &spec.home_dir, &spec.cache_dir.join("tmp")] {
            fs::create_dir_all(d).map_err(|e| EngineError::new(codes::ENGINE_UNAVAILABLE, format!("mkdir {}: {e}", d.display())))?;
        }
        if let Some(p) = spec.log_path.parent() {
            fs::create_dir_all(p).map_err(|e| EngineError::new(codes::ENGINE_UNAVAILABLE, format!("mkdir {}: {e}", p.display())))?;
        }
        if !seed.path.is_dir() {
            return Err(EngineError::new(codes::SEED_NOT_FOUND, format!("Seed {} is not in the store", seed.hash)));
        }
        Ok(Prepared { workdir: seed.path.clone() })
    }

    #[cfg(unix)]
    fn start(&self, spec: &AppSpec, p: &Prepared, port: u16) -> Result<RunHandle> {
        use std::os::unix::process::CommandExt;
        let Some((prog, args)) = spec.argv.split_first() else {
            return Err(EngineError::new(codes::MANIFEST_INVALID, "host.start is empty"));
        };
        let mut cmd = k2_core::bg_proc::command(prog);
        cmd.args(args).current_dir(&p.workdir).env_clear();
        for (k, v) in clean_env(spec, port) {
            cmd.env(k, v);
        }
        let secrets_dir = crate::vault::publish_env::apply_granted(&mut cmd, &spec.service_id, &spec.workspace_id, &p.workdir);
        cmd.stdin(Stdio::null());
        let sink = if pipe_sink_wanted() {
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
            SINK_PIPE
        } else {
            // The App holds its own log (O_APPEND), so it outlives the
            // daemon that started it. Rotate a full one before it starts.
            rotate_if_needed(&spec.log_path);
            let open = || OpenOptions::new().create(true).append(true).open(&spec.log_path);
            let (out, err) = match open().and_then(|f| f.try_clone().map(|g| (f, g))) {
                Ok(pair) => pair,
                Err(e) => {
                    return Err(EngineError::new(
                        codes::ENGINE_UNAVAILABLE,
                        format!("open the App's log {}: {e}", spec.log_path.display()),
                    ))
                }
            };
            cmd.stdout(Stdio::from(out)).stderr(Stdio::from(err));
            SINK_FILE
        };
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn().map_err(|e| {
            EngineError::new(codes::ENGINE_UNAVAILABLE, format!("start {:?} in {}: {e}", spec.argv, p.workdir.display()))
        })?;
        let pid = child.id() as i32;
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&spec.log_path) {
            let _ = writeln!(f, "[k2] deployment {} starting on port {port} (pid {pid})", spec.deployment_id);
        }
        if let (Some(o), Some(e)) = (child.stdout.take(), child.stderr.take()) {
            pump(o, spec.log_path.clone());
            pump(e, spec.log_path.clone());
        }
        if sink == SINK_FILE {
            watch_file_log(&spec.deployment_id, &spec.log_path);
        }
        let rf = RunFile { app_id: spec.app_id.clone(), deployment_id: spec.deployment_id.clone(), port, pid, log: sink.to_string() };
        let _ = fs::create_dir_all(run_dir(&spec.app_id));
        let _ = fs::write(run_file(&spec.app_id, &spec.deployment_id), serde_json::to_vec(&rf).unwrap_or_default());
        children()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(spec.deployment_id.clone(), Live { child: Some(child), secrets_dir });
        Ok(RunHandle {
            app_id: spec.app_id.clone(),
            deployment_id: spec.deployment_id.clone(),
            port,
            pid: Some(pid),
            engine_ref: format!("process:{pid}"),
        })
    }

    /// Windows: as on Unix, with a Job Object for the setsid group (stop
    /// ends the tree; it also ends with the daemon, so Windows doesn't
    /// adopt: the supervisor starts the App again at boot), npm-style
    /// `.cmd` programs run through cmd.exe, and the system variables a
    /// Windows program needs added to the clean environment.
    #[cfg(windows)]
    fn start(&self, spec: &AppSpec, p: &Prepared, port: u16) -> Result<RunHandle> {
        let Some((prog, args)) = spec.argv.split_first() else {
            return Err(EngineError::new(codes::MANIFEST_INVALID, "host.start is empty"));
        };
        let env = clean_env(spec, port);
        let path = env.iter().find(|(k, _)| k == "PATH").map(|(_, v)| v.clone()).unwrap_or_default();
        let (prog, args) = k2_core::terminal::win_cmd::resolve_spawn(prog, args, &path);
        let mut cmd = k2_core::bg_proc::command(&prog);
        cmd.args(&args).current_dir(&p.workdir).env_clear();
        for (k, v) in windows_base_env(spec).into_iter().chain(env) {
            cmd.env(k, v);
        }
        let secrets_dir = crate::vault::publish_env::apply_granted(&mut cmd, &spec.service_id, &spec.workspace_id, &p.workdir);
        cmd.stdin(Stdio::null());
        let sink = if pipe_sink_wanted() {
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
            SINK_PIPE
        } else {
            rotate_if_needed(&spec.log_path);
            let open = || OpenOptions::new().create(true).append(true).open(&spec.log_path);
            let (out, err) = match open().and_then(|f| f.try_clone().map(|g| (f, g))) {
                Ok(pair) => pair,
                Err(e) => {
                    return Err(EngineError::new(
                        codes::ENGINE_UNAVAILABLE,
                        format!("open the App's log {}: {e}", spec.log_path.display()),
                    ))
                }
            };
            cmd.stdout(Stdio::from(out)).stderr(Stdio::from(err));
            SINK_FILE
        };
        let (mut child, job) = crate::publish_runtime::win_job::spawn_in_job(&mut cmd).map_err(|e| {
            EngineError::new(codes::ENGINE_UNAVAILABLE, format!("start {:?} in {}: {e}", spec.argv, p.workdir.display()))
        })?;
        let pid = child.id() as i32;
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&spec.log_path) {
            let _ = writeln!(f, "[k2] deployment {} starting on port {port} (pid {pid})", spec.deployment_id);
        }
        if let (Some(o), Some(e)) = (child.stdout.take(), child.stderr.take()) {
            pump(o, spec.log_path.clone());
            pump(e, spec.log_path.clone());
        }
        if sink == SINK_FILE {
            watch_file_log(&spec.deployment_id, &spec.log_path);
        }
        let rf = RunFile { app_id: spec.app_id.clone(), deployment_id: spec.deployment_id.clone(), port, pid, log: sink.to_string() };
        let _ = fs::create_dir_all(run_dir(&spec.app_id));
        let _ = fs::write(run_file(&spec.app_id, &spec.deployment_id), serde_json::to_vec(&rf).unwrap_or_default());
        jobs().lock().unwrap_or_else(|e| e.into_inner()).insert(spec.deployment_id.clone(), job);
        children()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(spec.deployment_id.clone(), Live { child: Some(child), secrets_dir });
        Ok(RunHandle {
            app_id: spec.app_id.clone(),
            deployment_id: spec.deployment_id.clone(),
            port,
            pid: Some(pid),
            engine_ref: format!("process:{pid}"),
        })
    }

    #[cfg(not(any(unix, windows)))]
    fn start(&self, _spec: &AppSpec, _p: &Prepared, _port: u16) -> Result<RunHandle> {
        Err(EngineError::new(codes::ENGINE_UNAVAILABLE, "the process engine runs on Unix and Windows hosts only"))
    }

    fn liveness(&self, h: &RunHandle) -> Liveness {
        if let Some(code) = self.reap(&h.deployment_id) {
            return Liveness::Exited(code);
        }
        #[cfg(unix)]
        {
            match h.pid {
                Some(pid) if pid_alive(pid) => Liveness::Running,
                Some(_) => Liveness::Exited(None),
                None => Liveness::Unknown,
            }
        }
        // Windows: only our own child counts (reap above answered for an
        // exited one); a pid from before a restart may belong to anyone now.
        #[cfg(windows)]
        {
            let ours = children().lock().unwrap_or_else(|e| e.into_inner()).get(&h.deployment_id).is_some_and(|l| l.child.is_some());
            match h.pid {
                Some(_) if ours => Liveness::Running,
                Some(_) => Liveness::Exited(None),
                None => Liveness::Unknown,
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            Liveness::Unknown
        }
    }

    fn stop(&self, h: &RunHandle, grace: Duration) -> Result<()> {
        #[cfg(unix)]
        if let Some(pid) = h.pid {
            if pid_alive(pid) {
                signal_group(pid, libc::SIGTERM);
            }
            let end = Instant::now() + grace;
            while !self.exited(h) && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(50));
            }
            // Stragglers in the group (and a leader that ignored SIGTERM).
            // A child we spawned leads its own group (setsid), so its
            // group id is ours to kill even after the leader was reaped.
            let spawned_here = children()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&h.deployment_id)
                .is_some_and(|l| l.child.is_some());
            if spawned_here {
                unsafe {
                    let _ = libc::killpg(pid, libc::SIGKILL);
                }
            } else if pid_alive(pid) && own_group(pid) {
                signal_group(pid, libc::SIGKILL);
            }
            let live = children().lock().unwrap_or_else(|e| e.into_inner()).remove(&h.deployment_id);
            if let Some(mut l) = live {
                if let Some(mut c) = l.child.take() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                if let Some(d) = l.secrets_dir.take() {
                    let _ = fs::remove_dir_all(d);
                }
            }
        }
        // Windows has no SIGTERM for a windowless program: the job ends the
        // whole tree at once, so there is no grace period to wait out.
        #[cfg(windows)]
        {
            let _ = grace;
            // Only through our own job: never a bare pid, which after a
            // restart may be any process on the machine.
            if let Some(job) = jobs().lock().unwrap_or_else(|e| e.into_inner()).remove(&h.deployment_id) {
                job.terminate();
            }
            let end = std::time::Instant::now() + Duration::from_secs(2);
            while !self.exited(h) && std::time::Instant::now() < end {
                std::thread::sleep(Duration::from_millis(50));
            }
            let live = children().lock().unwrap_or_else(|e| e.into_inner()).remove(&h.deployment_id);
            if let Some(mut l) = live {
                if let Some(mut c) = l.child.take() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                if let Some(d) = l.secrets_dir.take() {
                    let _ = fs::remove_dir_all(d);
                }
            }
        }
        self.forget(h);
        Ok(())
    }

    fn exec(&self, _h: &RunHandle, _argv: &[String], _tty: bool) -> Result<ExecStream> {
        Err(EngineError::new(
            codes::ENGINE_UNAVAILABLE,
            "k2 app exec runs through the walled engines (k2-app-engine, a later 0.46 release)",
        ))
    }

    fn stats(&self, _h: &RunHandle) -> Result<CgroupStats> {
        Ok(CgroupStats::default())
    }

    fn adopt(&self) -> Result<Vec<RunHandle>> {
        let mut out = Vec::new();
        let Ok(apps) = fs::read_dir(k2_core::paths::k2_home().join("apps")) else { return Ok(out) };
        for a in apps.flatten() {
            let Ok(runs) = fs::read_dir(a.path().join("run")) else { continue };
            for r in runs.flatten() {
                let Ok(bytes) = fs::read(r.path()) else { continue };
                let Ok(rf) = serde_json::from_slice::<RunFile>(&bytes) else {
                    let _ = fs::remove_file(r.path());
                    continue;
                };
                #[cfg(unix)]
                let alive = pid_alive(rf.pid) && own_group(rf.pid);
                #[cfg(not(unix))]
                let alive = false;
                if !alive {
                    let _ = fs::remove_file(r.path());
                    continue;
                }
                children()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(rf.deployment_id.clone(), Live { child: None, secrets_dir: None });
                if rf.log == SINK_FILE {
                    watch_file_log(&rf.deployment_id, &deploy::deployment_log_path(&rf.app_id, &rf.deployment_id));
                } else {
                    stale_sinks().lock().unwrap_or_else(|e| e.into_inner()).insert(rf.deployment_id.clone());
                }
                out.push(RunHandle {
                    app_id: rf.app_id,
                    deployment_id: rf.deployment_id,
                    port: rf.port,
                    pid: Some(rf.pid),
                    engine_ref: format!("process:{}", rf.pid),
                });
            }
        }
        Ok(out)
    }
}

/// Windows: the process lane starts an App with its log as stdout and a
/// clean environment that still lets a Windows program run, reports it
/// running, and stop ends the whole tree (the job), not just the leader.
#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use k2_core::test_env::TempHome;
    use std::collections::BTreeMap;
    use std::time::Instant;

    #[test]
    fn a_windows_app_starts_logs_runs_and_stops_with_its_tree() {
        let h = TempHome::new();
        let seed_dir = h.path().join("seed");
        fs::create_dir_all(&seed_dir).unwrap();
        let app = deploy::app_dir("pe-win");
        let s = AppSpec {
            app_id: "pe-win".into(),
            service_id: "svc-pe-win".into(),
            workspace_id: "ws".into(),
            deployment_id: "d-win1".into(),
            isolation: Isolation::Process,
            // cmd prints, shows a system variable survived env_clear, then
            // waits in a grandchild (ping) the stop must end too.
            argv: ["cmd", "/d", "/c", "echo started %SystemRoot% %GREETING%& ping -n 59 127.0.0.1 >nul"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            env: BTreeMap::from([("GREETING".to_string(), "hi".to_string())]),
            memory_limit_bytes: None,
            drain_secs: 30,
            io_max_json: None,
            devices: vec![],
            public_origins: vec![],
            data_dir: app.join("data"),
            cache_dir: app.join("cache"),
            home_dir: app.join("home"),
            log_path: deploy::deployment_log_path("pe-win", "d-win1"),
            wall: Default::default(),
        };
        let e = ProcessEngine;
        let p = e.prepare(&SeedRef { hash: "x".into(), path: seed_dir }, &s).unwrap();
        let hdl = e.start(&s, &p, 1).unwrap();
        let end = Instant::now() + Duration::from_secs(10);
        let log = loop {
            let log = fs::read_to_string(&s.log_path).unwrap_or_default();
            if log.contains("started") {
                break log;
            }
            assert!(Instant::now() < end, "the App's stdout never reached its log: {log:?}");
            std::thread::sleep(Duration::from_millis(50));
        };
        let root = std::env::var("SystemRoot").expect("SystemRoot");
        assert!(log.contains(&format!("started {root} hi")), "{log}");
        assert_eq!(e.liveness(&hdl), Liveness::Running);
        assert!(run_file("pe-win", "d-win1").is_file());
        // This test's grandchild: the only ping counting to 59.
        let ours = || -> Vec<u32> {
            k2_core::win_proc::command_lines(&k2_core::win_proc::pids_named("PING"))
                .into_iter()
                .filter(|(_, c)| c.contains("-n 59 127.0.0.1"))
                .map(|(p, _)| p)
                .collect()
        };
        // cmd starts ping after it echoes: wait for it to appear.
        let end = Instant::now() + Duration::from_secs(10);
        while ours().len() != 1 {
            assert!(Instant::now() < end, "the grandchild runs: {:?}", ours());
            std::thread::sleep(Duration::from_millis(200));
        }
        e.stop(&hdl, Duration::from_secs(2)).unwrap();
        assert_ne!(e.liveness(&hdl), Liveness::Running);
        assert!(!run_file("pe-win", "d-win1").exists());
        let end = Instant::now() + Duration::from_secs(5);
        while !ours().is_empty() {
            assert!(Instant::now() < end, "stop must end the App's whole tree");
            std::thread::sleep(Duration::from_millis(200));
        }
        assert!(e.adopt().unwrap().is_empty(), "Windows doesn't adopt: the App ends with its job");
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use k2_core::test_env::TempHome;
    use std::collections::BTreeMap;

    fn spec(home: &Path, argv: &[&str]) -> AppSpec {
        let app = deploy::app_dir("pe-test");
        let _ = home;
        AppSpec {
            app_id: "pe-test".into(),
            service_id: "svc-pe".into(),
            workspace_id: "ws".into(),
            deployment_id: "d-pe1".into(),
            isolation: Isolation::Process,
            argv: argv.iter().map(|s| s.to_string()).collect(),
            env: BTreeMap::from([("GREETING".to_string(), "hi".to_string()), ("K2_HOOK_TOKEN".to_string(), "x".to_string())]),
            memory_limit_bytes: None,
            drain_secs: 30,
            io_max_json: None,
            devices: vec![],
            public_origins: vec![],
            data_dir: app.join("data"),
            cache_dir: app.join("cache"),
            home_dir: app.join("home"),
            log_path: deploy::deployment_log_path("pe-test", "d-pe1"),
            wall: Default::default(),
        }
    }

    #[test]
    fn clean_env_drops_daemon_secrets_and_keeps_the_allowlist() {
        let h = TempHome::new();
        let _tok = k2_core::test_env::EnvVar::set("K2_HOOK_TOKEN", "daemon-secret");
        let s = spec(h.path(), &["true"]);
        let env: BTreeMap<String, String> = clean_env_with_daemon_port(&s, 4321, 7777).into_iter().collect();
        assert_eq!(env.get("PORT").map(String::as_str), Some("4321"));
        assert_eq!(env.get("GREETING").map(String::as_str), Some("hi"));
        // AR37: K2_APP_ID is the service UUID (what a BFF sends on login);
        // the slug is K2_APP_NAME; K2_HOST/K2_PORT point at the daemon.
        assert_eq!(env.get("K2_APP_ID").map(String::as_str), Some("svc-pe"));
        assert_eq!(env.get("K2_APP_NAME").map(String::as_str), Some("pe-test"));
        assert_eq!(env.get("K2_HOST").map(String::as_str), Some("127.0.0.1"));
        assert_eq!(env.get("K2_PORT").map(String::as_str), Some("7777"));
        assert!(!env.contains_key("K2_HOOK_TOKEN"), "never the daemon's K2_* (nor a declared one)");
        assert!(!env.contains_key("K2_APP_ORIGIN") && !env.contains_key("K2_APP_ORIGINS"), "no public URL: unset");
        let mut with = spec(h.path(), &["true"]);
        with.public_origins = vec!["https://hearth.example.com".into(), "https://pe-test.sewlab.k2.dev".into()];
        let env: BTreeMap<String, String> = clean_env_with_daemon_port(&with, 4321, 7777).into_iter().collect();
        assert_eq!(env.get("K2_APP_ORIGIN").map(String::as_str), Some("https://hearth.example.com"));
        assert_eq!(env.get("K2_APP_ORIGINS").map(String::as_str), Some("https://hearth.example.com,https://pe-test.sewlab.k2.dev"));
        for k in env.keys() {
            assert!(
                ["PATH", "HOME", "PORT", "NODE_ENV", "TMPDIR", "LANG", "TZ", "GREETING", "K2_HOST", "K2_PORT"].contains(&k.as_str())
                    || k.starts_with("LC_")
                    || k.starts_with("K2_APP_"),
                "unexpected variable {k}"
            );
        }
    }

    #[test]
    fn start_liveness_stop_and_adopt() {
        let h = TempHome::new();
        let seed_dir = h.path().join("seed");
        fs::create_dir_all(&seed_dir).unwrap();
        let e = ProcessEngine;
        let s = spec(h.path(), &["sh", "-c", "echo started; exec sleep 30"]);
        let p = e.prepare(&SeedRef { hash: "x".into(), path: seed_dir.clone() }, &s).unwrap();
        let hdl = e.start(&s, &p, 1).unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        while !fs::read_to_string(&s.log_path).unwrap_or_default().contains("started") {
            assert!(Instant::now() < end, "the App's stdout never reached its log: {:?}", fs::read_to_string(&s.log_path));
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(e.liveness(&hdl), Liveness::Running);
        assert!(run_file("pe-test", "d-pe1").is_file());
        // A "daemon restart": forget the child, adopt from the run file.
        // (The test keeps the Child to reap it: after a real restart the
        // instance's parent is init, which reaps it.)
        let mut child = children().lock().unwrap().remove("d-pe1").unwrap().child.take().unwrap();
        let adopted = e.adopt().unwrap();
        assert_eq!(adopted.len(), 1);
        assert_eq!(adopted[0].pid, hdl.pid);
        let reaper = std::thread::spawn(move || child.wait().map(|s| s.code()));
        e.stop(&adopted[0], Duration::from_secs(2)).unwrap();
        let st = reaper.join().unwrap().expect("wait");
        assert_ne!(st, Some(0), "stopped by a signal, not a clean exit");
        assert!(!run_file("pe-test", "d-pe1").exists());
        assert!(e.adopt().unwrap().is_empty());
        let log = fs::read_to_string(&s.log_path).unwrap();
        assert!(log.contains("started"), "{log}");
    }

    fn wait_log(path: &Path, want: &str) -> String {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            let log = fs::read_to_string(path).unwrap_or_default();
            if log.contains(want) {
                return log;
            }
            assert!(Instant::now() < end, "{want:?} never reached the log: {log:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn run_json(app: &str, dep: &str) -> serde_json::Value {
        serde_json::from_slice(&fs::read(run_file(app, dep)).expect("run file")).expect("run file JSON")
    }

    /// Smoke r6: an unwalled App's stdout/stderr are its log FILE (held by
    /// the App), never a pipe into the daemon, so it keeps logging (and
    /// answering) when the daemon that started it is gone. The run file
    /// says so; adopting it needs no relaunch.
    #[test]
    fn the_app_holds_its_own_log_and_adopts_without_a_relaunch() {
        let h = TempHome::new();
        let _sink = k2_core::test_env::EnvVar::remove("K2_APPS_PROCESS_LOG");
        let seed_dir = h.path().join("seed");
        fs::create_dir_all(&seed_dir).unwrap();
        let e = ProcessEngine;
        let mut s = spec(h.path(), &["sh", "-c", "if [ -p /dev/stdout ]; then echo sink=pipe; else echo sink=notpipe; fi; echo oops >&2; exec sleep 30"]);
        s.deployment_id = "d-pe-file".into();
        s.log_path = deploy::deployment_log_path("pe-test", "d-pe-file");
        let p = e.prepare(&SeedRef { hash: "x".into(), path: seed_dir.clone() }, &s).unwrap();
        let hdl = e.start(&s, &p, 1).unwrap();
        let log = wait_log(&s.log_path, "oops");
        assert!(log.contains("sink=notpipe"), "stdout is the log file, not a pipe: {log}");
        assert_eq!(run_json("pe-test", "d-pe-file")["log"], "file");
        let mut child = children().lock().unwrap().remove("d-pe-file").unwrap().child.take().unwrap();
        let adopted = e.adopt().unwrap();
        assert_eq!(adopted.iter().map(|a| a.deployment_id.as_str()).collect::<Vec<_>>(), vec!["d-pe-file"]);
        assert!(!needs_relaunch("d-pe-file"), "a file sink survives the daemon");
        let reaper = std::thread::spawn(move || child.wait().map(|s| s.code()));
        e.stop(&adopted[0], Duration::from_secs(2)).unwrap();
        reaper.join().unwrap().expect("wait");
        assert!(!run_file("pe-test", "d-pe-file").exists());
        let _ = hdl;
    }

    /// A run file an older daemon wrote (no `log`: a pipe), or the
    /// `K2_APPS_PROCESS_LOG=pipe` way back: adopted, but marked for a
    /// relaunch (its pipe went with the daemon).
    #[test]
    fn a_pipe_sink_is_adopted_for_a_relaunch() {
        let h = TempHome::new();
        let _sink = k2_core::test_env::EnvVar::set("K2_APPS_PROCESS_LOG", "pipe");
        let seed_dir = h.path().join("seed");
        fs::create_dir_all(&seed_dir).unwrap();
        let e = ProcessEngine;
        let mut s = spec(h.path(), &["sh", "-c", "if [ -p /dev/stdout ]; then echo sink=pipe; else echo sink=notpipe; fi; exec sleep 30"]);
        s.deployment_id = "d-pe-pipe".into();
        s.log_path = deploy::deployment_log_path("pe-test", "d-pe-pipe");
        let p = e.prepare(&SeedRef { hash: "x".into(), path: seed_dir.clone() }, &s).unwrap();
        e.start(&s, &p, 1).unwrap();
        let log = wait_log(&s.log_path, "sink=");
        assert!(log.contains("sink=pipe"), "the way back pumps a pipe: {log}");
        assert_eq!(run_json("pe-test", "d-pe-pipe")["log"], "pipe");
        // An older daemon's run file: no `log` field at all.
        let mut v = run_json("pe-test", "d-pe-pipe");
        v.as_object_mut().unwrap().remove("log");
        fs::write(run_file("pe-test", "d-pe-pipe"), serde_json::to_vec(&v).unwrap()).unwrap();
        let mut child = children().lock().unwrap().remove("d-pe-pipe").unwrap().child.take().unwrap();
        let adopted = e.adopt().unwrap();
        assert_eq!(adopted.len(), 1);
        assert!(needs_relaunch("d-pe-pipe"), "no log field = a pipe to the old daemon");
        let reaper = std::thread::spawn(move || child.wait().map(|s| s.code()));
        e.stop(&adopted[0], Duration::from_secs(2)).unwrap();
        reaper.join().unwrap().expect("wait");
        assert!(!needs_relaunch("d-pe-pipe"), "forgotten once stopped");
    }

    /// Rotation of a log the App holds `O_APPEND`: copy to `.1`, truncate in
    /// place; the holder's next write lands in the emptied file.
    #[test]
    fn a_held_log_rotates_by_copy_and_truncate() {
        let h = TempHome::new();
        let path = h.path().join("held.log");
        let mut holder = OpenOptions::new().create(true).append(true).open(&path).unwrap();
        holder.write_all(&vec![b'a'; 4096]).unwrap();
        assert!(!rotate_held_log(&path, 8192), "under the cap: untouched");
        assert!(rotate_held_log(&path, 1024));
        assert_eq!(fs::metadata(path.with_extension("log.1")).unwrap().len(), 4096);
        assert_eq!(fs::metadata(&path).unwrap().len(), 0);
        holder.write_all(b"next\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "next\n", "the holder writes into the emptied file, not the copy");
    }
}
