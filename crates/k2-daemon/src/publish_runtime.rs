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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use k2_core::db::schema::SubdomainWorkspace;
use k2_core::log_debug;
use k2_core::published_services::{
    self as ps, ServiceJson, DESIRED_RUNNING, DESIRED_STOPPED, EXPOSE_LOCAL, EXPOSE_TUNNEL,
    STATUS_EXITED, STATUS_RUNNING, STATUS_STARTING, STATUS_STOPPED, STATUS_UNHEALTHY,
};
use k2_core::terminal::login_path;
use k2_core::tunnel::config as tunnel_config;
use k2_core::tunnel::subdomains;

const LOG_CAP: u64 = 2 * 1024 * 1024;
const KILL_GRACE: Duration = Duration::from_millis(400);
const DEFAULT_PROBE: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub struct PublishError {
    pub status: &'static str,
    pub message: String,
}

impl PublishError {
    fn bad(msg: impl Into<String>) -> Self {
        Self {
            status: "400 Bad Request",
            message: msg.into(),
        }
    }
    fn fail(msg: impl Into<String>) -> Self {
        Self {
            status: "400 Bad Request",
            message: msg.into(),
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

fn live_remove(project_id: &str, name: &str) -> Option<Arc<LiveChild>> {
    live()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&(project_id.to_string(), name.to_string()))
}

fn probe_timeout() -> Duration {
    std::env::var("K2_PUBLISH_PROBE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(|ms| Duration::from_millis(ms.max(50)))
        .unwrap_or(DEFAULT_PROBE)
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

fn pump_stdio(mut stdout: impl Read + Send + 'static, mut stderr: impl Read + Send + 'static, path: PathBuf) {
    let path_err = path.clone();
    thread::Builder::new()
        .name("publish-log-out".into())
        .spawn(move || copy_rotating(&mut stdout, &path))
        .ok();
    thread::Builder::new()
        .name("publish-log-err".into())
        .spawn(move || copy_rotating(&mut stderr, &path_err))
        .ok();
}

fn copy_rotating(src: &mut impl Read, path: &Path) {
    let mut buf = [0u8; 4096];
    loop {
        let n = match src.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        rotate_if_needed(path);
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = f.write_all(&buf[..n]);
        }
    }
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

fn kill_tree(entry: &LiveChild) {
    entry.stop_requested.store(true, Ordering::SeqCst);
    #[cfg(unix)]
    {
        let pid = entry.pid;
        if pid <= 0 {
            return;
        }
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
            job.terminate();
        } else {
            let _ = entry
                .child
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_mut()
                .map(|c| c.kill());
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

fn spawn_child(
    cmd: &str,
    cwd: &Path,
    project_id: &str,
    name: &str,
) -> Result<(Child, i32, Option<WinJobSlot>), String> {
    let _ = open_log(project_id, name)?;
    let log_p = log_path(project_id, name);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut c = Command::new("sh");
        c.arg("-c")
            .arg(cmd)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        enrich_path(&mut c);
        unsafe {
            c.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = c.spawn().map_err(|e| format!("spawn: {e}"))?;
        let pid = child.id() as i32;
        if let (Some(out), Some(err)) = (child.stdout.take(), child.stderr.take()) {
            pump_stdio(out, err, log_p);
        }
        Ok((child, pid, None))
    }
    #[cfg(windows)]
    {
        let mut c = Command::new("cmd.exe");
        c.arg("/c")
            .arg(cmd)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        enrich_path(&mut c);
        let (mut child, job) = win_job::spawn_in_job(&mut c)?;
        let pid = child.id() as i32;
        if let (Some(out), Some(err)) = (child.stdout.take(), child.stderr.take()) {
            pump_stdio(out, err, log_p);
        }
        Ok((child, pid, Some(job)))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (cmd, cwd, project_id, name, log_p);
        Err("published services require Unix or Windows".into())
    }
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

fn primary_host() -> String {
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
        if l.started.elapsed() < probe_timeout() {
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
    let (status, pid) = compute_status(row, live.as_deref());
    let url = if status == STATUS_STOPPED && row.error.is_some() && row.desired == DESIRED_STOPPED {
        // Hostname-fail path: do not advertise a public URL.
        if row.error.as_deref().is_some_and(|e| {
            e.contains("hostname")
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
    ServiceJson::from_row(row, status, url, pid)
}

fn load_row(project_id: &str, name: &str) -> Option<ps::PublishedService> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    ps::get_by_project_name(&conn, project_id, name).ok().flatten()
}

fn nested_preflight() -> Result<(tunnel_config::TunnelConfig, String), PublishError> {
    let cfg = tunnel_config::load().map_err(PublishError::fail)?;
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
         To host on this machine without a public URL: k2 publish run <name> --cmd \"…\" --port <n> --no-tunnel"
    )
}

fn attach_hostname(project_id: &str, name: &str, port: u16, token: &str, primary: &str) -> Result<(), String> {
    let target = format!("127.0.0.1:{port}");
    match subdomains::create_subdomain(token, name, &target) {
        Ok(()) => {}
        Err(e) if e.contains("label_taken") => {
            subdomains::point_subdomain(token, name, &target)?;
        }
        Err(e) => return Err(e),
    }
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        SubdomainWorkspace::claim(&conn, name, project_id).map_err(|e| e.to_string())?;
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
) -> Arc<LiveChild> {
    let entry = Arc::new(LiveChild {
        project_id: project_id.to_string(),
        name: name.to_string(),
        pid,
        port,
        started: Instant::now(),
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
    loop {
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
    loop {
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
    live_remove(&entry.project_id, &entry.name);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = ps::mark_exited(&conn, &entry.project_id, &entry.name, Some(code));
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
}

pub fn run(spec: RunSpec) -> Result<ServiceJson, PublishError> {
    let name = ps::normalize_name(&spec.name).map_err(PublishError::bad)?;
    if spec.cmd.trim().is_empty() {
        return Err(PublishError::bad("Missing cmd"));
    }
    if spec.port == 0 {
        return Err(PublishError::bad("Missing port"));
    }
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
                spec.cmd.trim(),
                &spec.cwd.to_string_lossy(),
                spec.port,
                expose,
            );
            let _ = ps::set_desired(&conn, &spec.project_id, &name, DESIRED_STOPPED);
            let _ = ps::set_error(&conn, &spec.project_id, &name, None);
        } else {
            ps::insert(
                &conn,
                &spec.project_id,
                &name,
                spec.cmd.trim(),
                &spec.cwd.to_string_lossy(),
                spec.port,
                expose,
                DESIRED_STOPPED,
            )
            .map_err(|e| {
                if ps::is_unique_violation(&e) {
                    PublishError::bad("service name already exists")
                } else {
                    PublishError::fail(e.to_string())
                }
            })?;
        }
    }

    match start_inner(&spec.project_id, &name, expose == EXPOSE_TUNNEL) {
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

pub fn start(project_id: &str, name: &str) -> Result<ServiceJson, PublishError> {
    let name = ps::normalize_name(name).map_err(PublishError::bad)?;
    let row = load_row(project_id, &name).ok_or_else(|| PublishError::bad("no such published service"))?;
    if is_running_now(project_id, &name, &row) {
        return Ok(to_json(&row));
    }
    if row.expose == EXPOSE_TUNNEL {
        nested_preflight()?;
    }
    start_inner(project_id, &name, row.expose == EXPOSE_TUNNEL)
}

fn start_inner(project_id: &str, name: &str, want_tunnel: bool) -> Result<ServiceJson, PublishError> {
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

    let cwd = PathBuf::from(&row.cwd);
    let (child, pid, job) = spawn_child(&row.cmd, &cwd, project_id, name).map_err(PublishError::fail)?;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = ps::set_runtime(&conn, project_id, name, Some(pid as i64), None, Some(now_unix()));
    }
    let entry = adopt_live(project_id, name, row.port as u16, child, pid, job);
    emit_changed(project_id);

    if !wait_for_port(row.port as u16, probe_timeout()) {
        entry.probe_ok.store(false, Ordering::SeqCst);
        if want_tunnel {
            rollback_hostname_fail(project_id, name, &entry, "probe failed: 127.0.0.1 port never accepted");
            return Err(PublishError::fail(
                "probe failed: process started but 127.0.0.1:<port> never accepted",
            ));
        }
        // Local: keep the process (status unhealthy). Persist desired=running
        // so boot still honors it.
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = ps::set_desired(&conn, project_id, name, DESIRED_RUNNING);
        }
        emit_changed(project_id);
        let row = load_row(project_id, name).unwrap_or(row);
        return Ok(to_json(&row));
    }
    entry.probe_ok.store(true, Ordering::SeqCst);

    if want_tunnel {
        let cfg = tunnel_config::load().map_err(|e| {
            rollback_hostname_fail(project_id, name, &entry, &e);
            PublishError::fail(e)
        })?;
        if let Err(e) = attach_hostname(project_id, name, row.port as u16, &cfg.token, &cfg.subdomain)
        {
            rollback_hostname_fail(project_id, name, &entry, &e);
            return Err(PublishError::fail(e));
        }
    }

    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = ps::set_desired(&conn, project_id, name, DESIRED_RUNNING);
        let _ = ps::set_error(&conn, project_id, name, None);
    }
    emit_changed(project_id);
    let row = load_row(project_id, name).unwrap_or(row);
    Ok(to_json(&row))
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
    let name = ps::normalize_name(name).map_err(PublishError::bad)?;
    let row = load_row(project_id, &name).ok_or_else(|| PublishError::bad("no such published service"))?;
    if let Some(entry) = live_remove(project_id, &name) {
        kill_tree(&entry);
    } else if let Some(pid) = row.pid.and_then(|p| i32::try_from(p).ok()) {
        // Reattached / unknown live pid: kill the group we own if pgid==pid.
        kill_pid_tree(pid);
    }
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = ps::set_desired(&conn, project_id, &name, DESIRED_STOPPED);
        let _ = ps::mark_exited(&conn, project_id, &name, None);
        let _ = ps::set_runtime(&conn, project_id, &name, None, row.error.as_deref(), None);
    }
    emit_changed(project_id);
    let row = load_row(project_id, &name).unwrap_or(row);
    Ok(to_json(&row))
}

fn kill_pid_tree(pid: i32) {
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
    #[cfg(windows)]
    {
        win_job::kill_pid(pid as u32);
    }
}

pub fn rm(project_id: &str, name: &str, keep_hostname: bool) -> Result<(), PublishError> {
    let name = ps::normalize_name(name).map_err(PublishError::bad)?;
    let row = load_row(project_id, &name);
    let _ = stop(project_id, &name);
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = SubdomainWorkspace::unclaim(&conn, &name);
        let _ = ps::delete(&conn, project_id, &name);
    }
    if !keep_hostname {
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

/// Boot: desired=running → reattach if pid alive AND port accepts, else respawn.
/// Stagger between services. Bind clash fails loud (error on the row).
pub fn boot_desired_running() {
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ps::list_desired_running(&conn).unwrap_or_default()
    };
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
            adopt_reattach(&row.project_id, &row.name, port, pid);
            emit_changed(&row.project_id);
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
        if let Err(e) = start_inner(&row.project_id, &row.name, row.expose == EXPOSE_TUNNEL) {
            log_debug!(
                "[publish] boot respawn {}/{} failed: {}",
                row.project_id,
                row.name,
                e.message
            );
        }
    }
}

/// Long-lived supervisor thread: boot respawn then idle (child waiters
/// are per-service threads).
pub fn spawn() -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("publish-runtime".into())
        .spawn(|| {
            boot_desired_running();
            loop {
                thread::sleep(Duration::from_secs(30));
            }
        })
        .expect("spawn publish-runtime")
}

#[cfg(windows)]
mod win_job {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command};

    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
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
        fn TerminateProcess(h: *mut core::ffi::c_void, code: u32) -> i32;
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
    }

    pub fn spawn_in_job(cmd: &mut Command) -> Result<(Child, Job), String> {
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
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

    pub fn kill_pid(pid: u32) {
        const PROCESS_TERMINATE: u32 = 0x0001;
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if !h.is_null() {
                TerminateProcess(h, 1);
                CloseHandle(h);
            }
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomOrd};
    use std::sync::Arc as StdArc;

    fn unique(prefix: &str) -> String {
        format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    fn make_project(path: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT OR IGNORE INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, "pub", path],
        )
        .expect("insert project");
        id
    }

    fn cleanup(project_id: &str, name: &str) {
        let _ = stop(project_id, name);
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = ps::delete(&conn, project_id, name);
        let _ = k2_core::db::schema::SubdomainWorkspace::unclaim(&conn, name);
    }

    fn listener_cmd(port: u16) -> String {
        format!(
            "python3 -c \"import socket,itertools; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); s.bind(('127.0.0.1',{port})); s.listen(8); [s.accept()[0].close() for _ in itertools.count()]\""
        )
    }

    fn free_port() -> u16 {
        TcpListener::bind(("127.0.0.1", 0))
            .expect("bind")
            .local_addr()
            .unwrap()
            .port()
    }

    fn python_ok() -> bool {
        Command::new("python3")
            .arg("-c")
            .arg("print(1)")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    struct MockCp {
        hits: StdArc<AtomicUsize>,
        _join: thread::JoinHandle<()>,
        prev: Option<std::ffi::OsString>,
    }

    impl Drop for MockCp {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(p) => std::env::set_var(subdomains::CONTROL_PLANE_BASE_ENV, p),
                None => std::env::remove_var(subdomains::CONTROL_PLANE_BASE_ENV),
            }
        }
    }

    fn start_mock_cp(status: u16, body: &'static str, methods_ok: &'static str) -> MockCp {
        let hits = StdArc::new(AtomicUsize::new(0));
        let hits2 = StdArc::clone(&hits);
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("mock cp");
        let port = listener.local_addr().unwrap().port();
        let join = thread::spawn(move || {
            listener.set_nonblocking(true).ok();
            let deadline = Instant::now() + Duration::from_secs(20);
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut sock, _)) => {
                        hits2.fetch_add(1, AtomOrd::SeqCst);
                        let mut buf = [0u8; 4096];
                        let _ = sock.read(&mut buf);
                        let req = String::from_utf8_lossy(&buf);
                        let allow = methods_ok == "*" || req.starts_with(methods_ok);
                        let (code, payload) = if allow {
                            (status, body)
                        } else {
                            (500, "{\"error\":\"unexpected\"}")
                        };
                        let resp = format!(
                            "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                            payload.len()
                        );
                        let _ = sock.write_all(resp.as_bytes());
                    }
                    Err(_) => thread::sleep(Duration::from_millis(20)),
                }
            }
        });
        let prev = std::env::var_os(subdomains::CONTROL_PLANE_BASE_ENV);
        std::env::set_var(
            subdomains::CONTROL_PLANE_BASE_ENV,
            format!("http://127.0.0.1:{port}"),
        );
        MockCp {
            hits,
            _join: join,
            prev,
        }
    }

    #[test]
    fn spawn_probe_and_stop_kills_tree() {
        if !python_ok() {
            panic!("python3 is required for published-service runtime tests");
        }
        let _home = crate::test_support::TempHome::new();
        std::env::set_var("K2_PUBLISH_PROBE_MS", "4000");
        let dir = _home.path().join("ws");
        fs::create_dir_all(&dir).unwrap();
        let project_id = make_project(&dir.to_string_lossy());
        let name = unique("tree");
        let port = free_port();
        // Grandchild: python listener in the background of the shell.
        let cmd = format!("{} & wait", listener_cmd(port));
        let spec = RunSpec {
            project_id: project_id.clone(),
            name: name.clone(),
            cmd,
            cwd: dir,
            port,
            no_tunnel: true,
            replace_spec: false,
        };
        let json = run(spec).expect("run");
        assert_eq!(
            json.status, STATUS_RUNNING,
            "probe must succeed; pid={:?} port_up={}",
            json.pid,
            port_accepts(port)
        );
        assert!(json.pid.is_some());
        assert!(json.url.is_none(), "no-tunnel has no public URL");
        let pid = json.pid.unwrap() as i32;
        assert!(pid_alive(pid), "shell still alive");
        assert!(
            crate::v2_session_map::snapshot()
                .iter()
                .all(|(_, s)| s.child_pid() != Some(pid)),
            "published pid must not be in v2_session_map"
        );
        stop(&project_id, &name).expect("stop");
        thread::sleep(Duration::from_millis(300));
        assert!(!pid_alive(pid), "stop must kill the process group");
        assert!(!port_accepts(port), "listener grandchild must die too");
        let after = load_row(&project_id, &name).unwrap();
        assert_eq!(after.desired, DESIRED_STOPPED);
        cleanup(&project_id, &name);
    }

    #[test]
    fn no_tunnel_never_hits_control_plane() {
        if !python_ok() {
            panic!("python3 is required for published-service runtime tests");
        }
        let _home = crate::test_support::TempHome::new();
        std::env::set_var("K2_PUBLISH_PROBE_MS", "4000");
        let mock = start_mock_cp(200, r#"{"subdomains":[]}"#, "*");
        let dir = _home.path().join("ws2");
        fs::create_dir_all(&dir).unwrap();
        let project_id = make_project(&dir.to_string_lossy());
        let name = unique("local");
        let port = free_port();
        let json = run(RunSpec {
            project_id: project_id.clone(),
            name: name.clone(),
            cmd: listener_cmd(port),
            cwd: dir,
            port,
            no_tunnel: true,
            replace_spec: false,
        })
        .expect("run --no-tunnel");
        assert_eq!(json.expose, EXPOSE_LOCAL);
        assert_eq!(json.status, STATUS_RUNNING, "no-tunnel must actually listen");
        assert_eq!(
            mock.hits.load(AtomOrd::SeqCst),
            0,
            "--no-tunnel must never dial connect.k2.dev (GET or POST)"
        );
        cleanup(&project_id, &name);
    }

    #[test]
    fn nested_without_pro_fails_before_spawn() {
        let _home = crate::test_support::TempHome::new();
        // Persist a token so the gate reaches the Pro check (and not
        // "no token") — still no spawn.
        let cfg = serde_json::json!({
            "token": "tok_test",
            "subdomain": "rosson",
            "e2e": true
        });
        fs::write(_home.path().join(".k2/tunnel.json"), cfg.to_string()).unwrap();
        let _mock = start_mock_cp(
            200,
            r#"{"subdomains":[{"label":"rosson","primary":true,"connected":true,"tier":"free"}]}"#,
            "GET ",
        );
        let dir = _home.path().join("ws3");
        fs::create_dir_all(&dir).unwrap();
        let project_id = make_project(&dir.to_string_lossy());
        let name = unique("nopro");
        let marker = dir.join("spawned");
        let cmd = format!("echo hi > {}", marker.display());
        let err = run(RunSpec {
            project_id: project_id.clone(),
            name: name.clone(),
            cmd,
            cwd: dir.clone(),
            port: 9,
            no_tunnel: false,
            replace_spec: false,
        })
        .expect_err("not Pro must fail");
        assert!(
            err.message.contains("pro_required") || err.message.contains("dashboard"),
            "got: {}",
            err.message
        );
        assert!(
            err.message.contains("--no-tunnel"),
            "hint --no-tunnel: {}",
            err.message
        );
        assert!(!marker.exists(), "must not spawn");
        assert!(
            load_row(&project_id, &name).is_none(),
            "no row on pre-spawn fail"
        );
        cleanup(&project_id, &name);
    }

    #[test]
    fn hostname_fail_does_not_leave_desired_running() {
        if !python_ok() {
            panic!("python3 is required for published-service runtime tests");
        }
        let _home = crate::test_support::TempHome::new();
        std::env::set_var("K2_PUBLISH_PROBE_MS", "4000");
        std::env::set_var("K2_PUBLISH_CLAIM_TRIES", "2");
        let cfg = serde_json::json!({
            "token": "tok_test",
            "subdomain": "rosson",
            "e2e": true
        });
        fs::write(_home.path().join(".k2/tunnel.json"), cfg.to_string()).unwrap();
        // GET = Pro; POST create fails.
        let hits = StdArc::new(AtomicUsize::new(0));
        let hits2 = StdArc::clone(&hits);
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("mock");
        let port_cp = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            listener.set_nonblocking(true).ok();
            let deadline = Instant::now() + Duration::from_secs(20);
            while Instant::now() < deadline {
                if let Ok((mut sock, _)) = listener.accept() {
                    hits2.fetch_add(1, AtomOrd::SeqCst);
                    let mut buf = [0u8; 8192];
                    let n = sock.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let (code, body) = if req.starts_with("GET ") {
                        (
                            200,
                            r#"{"subdomains":[{"label":"rosson","primary":true,"connected":true,"tier":"pro"}]}"#,
                        )
                    } else {
                        (403, r#"{"error":"pro_required"}"#)
                    };
                    let resp = format!(
                        "HTTP/1.1 {code} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes());
                } else {
                    thread::sleep(Duration::from_millis(20));
                }
            }
        });
        let prev = std::env::var_os(subdomains::CONTROL_PLANE_BASE_ENV);
        std::env::set_var(
            subdomains::CONTROL_PLANE_BASE_ENV,
            format!("http://127.0.0.1:{port_cp}"),
        );

        let dir = _home.path().join("ws4");
        fs::create_dir_all(&dir).unwrap();
        let project_id = make_project(&dir.to_string_lossy());
        let name = unique("hostfail");
        let port = free_port();
        let err = run(RunSpec {
            project_id: project_id.clone(),
            name: name.clone(),
            cmd: listener_cmd(port),
            cwd: dir,
            port,
            no_tunnel: false,
            replace_spec: false,
        })
        .expect_err("hostname fail");
        match prev {
            Some(p) => std::env::set_var(subdomains::CONTROL_PLANE_BASE_ENV, p),
            None => std::env::remove_var(subdomains::CONTROL_PLANE_BASE_ENV),
        }
        assert!(
            !err.message.contains("probe failed"),
            "must get past probe to the hostname step: {}",
            err.message
        );
        assert!(!err.message.is_empty());
        assert!(!port_accepts(port), "child must be stopped");
        if let Some(row) = load_row(&project_id, &name) {
            assert_ne!(row.desired, DESIRED_RUNNING, "must not persist desired=running");
            assert_eq!(row.expose, EXPOSE_TUNNEL);
            assert!(row.pid.is_none());
        }
        // Simulated boot must not start it local-only.
        boot_desired_running();
        assert!(
            !port_accepts(port),
            "boot must not resurrect hostname-failed as local"
        );
        cleanup(&project_id, &name);
    }

    #[test]
    fn run_same_name_while_running_is_400() {
        if !python_ok() {
            panic!("python3 is required");
        }
        let _home = crate::test_support::TempHome::new();
        std::env::set_var("K2_PUBLISH_PROBE_MS", "4000");
        let dir = _home.path().join("ws5");
        fs::create_dir_all(&dir).unwrap();
        let project_id = make_project(&dir.to_string_lossy());
        let name = unique("dup");
        let port = free_port();
        let spec = || RunSpec {
            project_id: project_id.clone(),
            name: name.clone(),
            cmd: listener_cmd(port),
            cwd: dir.clone(),
            port,
            no_tunnel: true,
            replace_spec: false,
        };
        run(spec()).expect("first run");
        let err = run(spec()).expect_err("second must fail");
        assert!(err.message.contains("stop first"), "got: {}", err.message);
        cleanup(&project_id, &name);
    }

    #[test]
    fn windows_job_objects_required() {
        #[cfg(windows)]
        {
            let mut cmd = Command::new("cmd.exe");
            cmd.args(["/c", "ping", "-n", "3", "127.0.0.1"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            win_job::spawn_in_job(&mut cmd).expect("Job Objects are required on Windows");
        }
    }
}
