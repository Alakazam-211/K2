//! Skin front door — Caddy path-filter (Vein 2) + nested `skin.<sub>`.
//!
//! Pure Caddyfile renderer plus apply orchestration (write config, supervise
//! Caddy, register the reserved nested label). The daemon HTTP listener stays
//! on loopback; this never sets `K2_LISTEN=lan`.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::skin::{self, SkinFrontDoor};
use crate::tunnel::config::SUBDOMAIN_HOST;

/// Loopback listen for nested Internal + localhost. Not 38471 (air-gap), not 443.
pub const LOOPBACK_PORT: u16 = 38472;

/// Nested label reserved for the Skin door (`https://skin.<sub>.k2.dev`).
pub const NESTED_LABEL: &str = skin::RESERVED_NESTED_LABEL;

/// Shown when Connect has no public URL yet.
pub const CONNECT_URL_STUB: &str = "https://skin.<sub>.k2.dev";

pub const PATH_FILTER_ERROR: &str = "skin front door does not proxy this path";

pub const CADDY_MISSING: &str = "caddy_missing: the Skin front door needs Caddy on PATH. \
Install: brew install caddy (macOS) or sudo apt install caddy (Debian/Ubuntu). \
Do not bind k2-daemon to the world (never K2_LISTEN=lan).";

const DIRECT_443_FALLBACK_HINT: &str = "Could not bind :443 (need root/CAP_NET_BIND_SERVICE). \
Skin Direct is listening on 0.0.0.0:38472. DNS has no port; production Direct wants 443. \
Do not bind k2-daemon to the world.";

const NO_PUBLIC_NESTED_HINT: &str = "No public nested URL (air-gap or no Connect token). \
Loopback Caddy is up at 127.0.0.1:38472. Operator <sub>.k2.dev stays the kingdom door.";

// ── Types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CaddyStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub binary: Option<String>,
    pub config_path: String,
    pub missing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NestedStatus {
    pub label: String,
    pub host: Option<String>,
    pub target: Option<String>,
    pub registered: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkinFrontDoorStatus {
    pub mode: String,
    pub url: Option<String>,
    pub hint: Option<String>,
    pub connect_url: String,
    pub listen: String,
    pub ui_port: Option<u16>,
    pub applied: bool,
    pub caddy: CaddyStatus,
    pub nested: NestedStatus,
    pub error: Option<String>,
}

/// Inputs for the pure Caddyfile renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaddyfileSpec {
    pub daemon_port: u16,
    pub loopback_port: u16,
    /// Direct-mode extra bind. `None` = connect (loopback only).
    pub extra_listen: Option<String>,
    pub ui_port: Option<u16>,
}

struct LiveCaddy {
    child: Child,
    pid: u32,
    binary: PathBuf,
    listen: String,
    nested: NestedStatus,
}

fn live() -> &'static Mutex<Option<LiveCaddy>> {
    static LIVE: OnceLock<Mutex<Option<LiveCaddy>>> = OnceLock::new();
    LIVE.get_or_init(|| Mutex::new(None))
}

// ── Paths ────────────────────────────────────────────────────────────

pub fn skin_dir() -> PathBuf {
    crate::paths::k2_home().join("skin")
}

pub fn caddyfile_path() -> PathBuf {
    skin_dir().join("Caddyfile")
}

pub fn caddy_pid_path() -> PathBuf {
    skin_dir().join("caddy.pid")
}

pub fn caddy_log_path() -> PathBuf {
    skin_dir().join("caddy.log")
}

fn ensure_skin_dir() -> Result<PathBuf, String> {
    let dir = skin_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

// ── Pure renderer ────────────────────────────────────────────────────

/// Render a Caddyfile that reverse-proxies Thread + overlay + boot-status
/// to the daemon on loopback, and **403s everything else** (especially grid,
/// PTY, login, `/v1`). Never copies the air-gap whole-daemon proxy.
pub fn render_caddyfile(spec: &CaddyfileSpec) -> String {
    let daemon = format!("127.0.0.1:{}", spec.daemon_port);
    let addrs = site_addresses(spec.loopback_port, spec.extra_listen.as_deref());
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("\tadmin off\n");
    out.push_str("\tauto_https off\n");
    out.push_str("}\n\n");
    out.push_str(&addrs);
    out.push_str(" {\n");
    push_handle(&mut out, "/boot-status*", &daemon);
    push_handle(&mut out, "/cli/thread", &daemon);
    push_handle(&mut out, "/cli/thread/*", &daemon);
    push_handle(&mut out, "/cli/overlay/events*", &daemon);
    if let Some(ui) = spec.ui_port {
        out.push_str("\t@skinUi path_regexp ^/$\n");
        out.push_str("\thandle @skinUi {\n");
        out.push_str("\t\treverse_proxy 127.0.0.1:");
        out.push_str(&ui.to_string());
        out.push('\n');
        out.push_str("\t}\n\n");
    }
    out.push_str("\thandle {\n");
    out.push_str("\t\theader Content-Type application/json\n");
    out.push_str("\t\trespond `{\"error\":\"");
    out.push_str(PATH_FILTER_ERROR);
    out.push_str("\"}` 403\n");
    out.push_str("\t}\n");
    out.push_str("}\n");
    out
}

fn push_handle(out: &mut String, matcher: &str, daemon: &str) {
    out.push_str("\thandle ");
    out.push_str(matcher);
    out.push_str(" {\n");
    out.push_str("\t\treverse_proxy ");
    out.push_str(daemon);
    out.push('\n');
    out.push_str("\t}\n\n");
}

fn site_addresses(loopback_port: u16, extra_listen: Option<&str>) -> String {
    let loopback = format!("http://127.0.0.1:{loopback_port}");
    match extra_listen.map(str::trim).filter(|s| !s.is_empty()) {
        None => loopback,
        Some(extra) if extra == ":443" || extra == "http://:443" => {
            format!("{loopback}, http://:443")
        }
        Some(extra) => {
            // 0.0.0.0:38472 covers loopback on the same port — do not bind twice.
            let hostport = extra.strip_prefix("http://").unwrap_or(extra);
            if hostport.starts_with("0.0.0.0:") {
                format!("http://{hostport}")
            } else {
                format!("{loopback}, http://{hostport}")
            }
        }
    }
}

// ── Bind probe ───────────────────────────────────────────────────────

pub fn can_bind(addr: &str) -> bool {
    TcpListener::bind(addr).is_ok()
}

pub fn can_bind_443() -> bool {
    can_bind("0.0.0.0:443") || can_bind("[::]:443")
}

fn direct_extra_listen() -> (String, Option<String>) {
    if can_bind_443() {
        (":443".to_string(), None)
    } else {
        (
            format!("0.0.0.0:{LOOPBACK_PORT}"),
            Some(DIRECT_443_FALLBACK_HINT.to_string()),
        )
    }
}

// ── Caddy binary ─────────────────────────────────────────────────────

fn which_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if is_executable(&cand) {
            return Some(cand);
        }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{name}.exe"));
            if is_executable(&exe) {
                return Some(exe);
            }
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && (m.permissions().mode() & 0o111 != 0))
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// PATH-only. Do not bundle Caddy; do not search brew prefixes off PATH.
pub fn resolve_caddy() -> Result<PathBuf, String> {
    which_in_path("caddy").ok_or_else(|| CADDY_MISSING.to_string())
}

fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

fn kill_pid(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        std::thread::sleep(Duration::from_millis(80));
        if pid_alive(pid) {
            let _ = Command::new("kill")
                .args(["-9", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn stray_caddy_pattern(cfg: &Path) -> String {
    format!("caddy run --config {}", cfg.display())
}

#[cfg(unix)]
fn reap_stray_caddy(cfg: &Path) {
    let _ = Command::new("pkill")
        .arg("-f")
        .arg(stray_caddy_pattern(cfg))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(unix))]
fn reap_stray_caddy(_cfg: &Path) {}

fn read_pid_file() -> Option<u32> {
    let raw = std::fs::read_to_string(caddy_pid_path()).ok()?;
    raw.trim().parse().ok()
}

fn write_pid_file(pid: u32) {
    let path = caddy_pid_path();
    let _ = std::fs::write(&path, format!("{pid}\n"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
}

fn spawn_caddy(bin: &Path, cfg: &Path) -> Result<Child, String> {
    let log_path = caddy_log_path();
    let mut cmd = Command::new(bin);
    cmd.arg("run")
        .arg("--config")
        .arg(cfg)
        .arg("--adapter")
        .arg("caddyfile")
        .stdin(Stdio::null());
    match std::fs::File::create(&log_path) {
        Ok(out) => match out.try_clone() {
            Ok(err) => {
                cmd.stdout(Stdio::from(out));
                cmd.stderr(Stdio::from(err));
            }
            Err(_) => {
                cmd.stdout(Stdio::from(out));
                cmd.stderr(Stdio::null());
            }
        },
        Err(_) => {
            cmd.stdout(Stdio::null());
            cmd.stderr(Stdio::null());
        }
    }
    cmd.spawn()
        .map_err(|e| format!("bind_failed: spawn caddy: {e}"))
}

/// Stop the supervised Caddy (pid file + live child + pattern reap).
pub fn stop_caddy() {
    if let Some(mut live_proc) = live().lock().take() {
        let _ = live_proc.child.kill();
        let _ = live_proc.child.wait();
        kill_pid(live_proc.pid);
    }
    if let Some(pid) = read_pid_file() {
        kill_pid(pid);
    }
    reap_stray_caddy(&caddyfile_path());
    let _ = std::fs::remove_file(caddy_pid_path());
}

fn restart_caddy(
    bin: &Path,
    cfg: &Path,
    listen: String,
    nested: NestedStatus,
) -> Result<(), String> {
    stop_caddy();
    let mut child = spawn_caddy(bin, cfg)?;
    let pid = child.id();
    write_pid_file(pid);
    std::thread::sleep(Duration::from_millis(150));
    match child.try_wait() {
        Ok(Some(status)) => {
            let log = std::fs::read_to_string(caddy_log_path()).unwrap_or_default();
            let tail: String = log
                .lines()
                .rev()
                .take(8)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" | ");
            return Err(format!(
                "bind_failed: caddy exited immediately ({status}). {tail}"
            ));
        }
        Ok(None) => {}
        Err(e) => {
            return Err(format!("bind_failed: caddy wait: {e}"));
        }
    }
    *live().lock() = Some(LiveCaddy {
        child,
        pid,
        binary: bin.to_path_buf(),
        listen,
        nested,
    });
    Ok(())
}

// ── Nested hostname ──────────────────────────────────────────────────

fn nested_target(loopback_port: u16) -> String {
    format!("127.0.0.1:{loopback_port}")
}

fn nested_host_from_cfg(cfg: &crate::tunnel::config::TunnelConfig) -> Option<String> {
    let sub = cfg.subdomain.trim();
    if sub.is_empty() {
        None
    } else {
        Some(format!("skin.{sub}.{SUBDOMAIN_HOST}"))
    }
}

fn skip_cp_register() -> bool {
    crate::airgap::enabled()
}

fn register_nested(loopback_port: u16) -> Result<NestedStatus, String> {
    let target = nested_target(loopback_port);
    let cfg = crate::tunnel::config::load().unwrap_or_default();
    let host = nested_host_from_cfg(&cfg);
    if skip_cp_register() || cfg.token.trim().is_empty() {
        return Ok(NestedStatus {
            label: NESTED_LABEL.to_string(),
            host,
            target: Some(target),
            registered: false,
        });
    }
    match crate::tunnel::subdomains::create_subdomain(&cfg.token, NESTED_LABEL, &target) {
        Ok(()) => {}
        Err(e) if e.contains("label_taken") => {
            crate::tunnel::subdomains::point_subdomain(&cfg.token, NESTED_LABEL, &target)?;
        }
        Err(e) => return Err(e),
    }
    Ok(NestedStatus {
        label: NESTED_LABEL.to_string(),
        host,
        target: Some(target),
        registered: true,
    })
}

fn empty_nested() -> NestedStatus {
    NestedStatus {
        label: NESTED_LABEL.to_string(),
        host: None,
        target: None,
        registered: false,
    }
}

// ── Status / apply ───────────────────────────────────────────────────

fn connect_url_of(door: &SkinFrontDoor) -> String {
    if let Some(u) = door.url.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return u.to_string();
    }
    CONNECT_URL_STUB.to_string()
}

fn default_listen(mode: &str) -> String {
    if mode == "direct" {
        format!("0.0.0.0:{LOOPBACK_PORT}")
    } else {
        format!("127.0.0.1:{LOOPBACK_PORT}")
    }
}

fn caddy_snapshot() -> CaddyStatus {
    let bin = resolve_caddy().ok();
    let missing = bin.is_none();
    let live_g = live().lock();
    if let Some(st) = live_g.as_ref() {
        let running = pid_alive(st.pid);
        return CaddyStatus {
            running,
            pid: running.then_some(st.pid),
            binary: Some(st.binary.display().to_string()),
            config_path: caddyfile_path().display().to_string(),
            missing: false,
        };
    }
    drop(live_g);
    if let Some(pid) = read_pid_file() {
        let running = pid_alive(pid);
        return CaddyStatus {
            running,
            pid: running.then_some(pid),
            binary: bin.map(|p| p.display().to_string()),
            config_path: caddyfile_path().display().to_string(),
            missing,
        };
    }
    CaddyStatus {
        running: false,
        pid: None,
        binary: bin.map(|p| p.display().to_string()),
        config_path: caddyfile_path().display().to_string(),
        missing,
    }
}

/// Snapshot for GET `/cli/skin/front-door`. Does not spawn Caddy.
pub fn status() -> Result<SkinFrontDoorStatus, String> {
    let door = skin::effective_front_door()?;
    let caddy = caddy_snapshot();
    let live_g = live().lock();
    let (applied, nested, listen) = if let Some(st) = live_g.as_ref() {
        (pid_alive(st.pid), st.nested.clone(), st.listen.clone())
    } else {
        (
            caddy.running,
            empty_nested_with_cfg(),
            default_listen(&door.mode),
        )
    };
    drop(live_g);
    let mut hint = door.hint.clone();
    let error = None;
    if door.mode == "direct" && listen.starts_with("0.0.0.0:") {
        hint = Some(DIRECT_443_FALLBACK_HINT.to_string());
    }
    if door.mode == "connect" && !nested.registered {
        let cfg = crate::tunnel::config::load().unwrap_or_default();
        if skip_cp_register() || cfg.token.trim().is_empty() {
            hint = Some(NO_PUBLIC_NESTED_HINT.to_string());
        }
    }
    Ok(SkinFrontDoorStatus {
        connect_url: connect_url_of(&door),
        mode: door.mode,
        url: door.url,
        hint,
        listen,
        ui_port: door.ui_port,
        applied,
        caddy,
        nested,
        error,
    })
}

fn empty_nested_with_cfg() -> NestedStatus {
    let cfg = crate::tunnel::config::load().unwrap_or_default();
    NestedStatus {
        label: NESTED_LABEL.to_string(),
        host: nested_host_from_cfg(&cfg),
        target: Some(nested_target(LOOPBACK_PORT)),
        registered: false,
    }
}

fn write_caddyfile(contents: &str) -> Result<PathBuf, String> {
    ensure_skin_dir()?;
    let path = caddyfile_path();
    std::fs::write(&path, contents).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path)
}

/// Persist is the caller's job. This writes the Caddyfile, (re)starts Caddy,
/// and registers nested `skin` when mode is connect and a tunnel token exists.
pub fn apply(daemon_port: u16) -> Result<SkinFrontDoorStatus, String> {
    if daemon_port == 0 {
        return Err("bind_failed: daemon port is 0".into());
    }
    let bin = resolve_caddy()?;
    let door = skin::effective_front_door()?;
    let (extra, direct_hint) = if door.mode == "direct" {
        let (listen, hint) = direct_extra_listen();
        (Some(listen), hint)
    } else {
        (None, None)
    };
    let spec = CaddyfileSpec {
        daemon_port,
        loopback_port: LOOPBACK_PORT,
        extra_listen: extra.clone(),
        ui_port: door.ui_port,
    };
    let rendered = render_caddyfile(&spec);
    let cfg_path = write_caddyfile(&rendered)?;
    let listen = match extra.as_deref() {
        None => format!("127.0.0.1:{LOOPBACK_PORT}"),
        Some(":443") => ":443".to_string(),
        Some(other) => other.to_string(),
    };
    // Caddy first (loopback door), then nested CP register — a Pro/token
    // failure still leaves the filtered loopback listener up.
    let nested_placeholder = if door.mode == "connect" {
        NestedStatus {
            label: NESTED_LABEL.to_string(),
            host: nested_host_from_cfg(&crate::tunnel::config::load().unwrap_or_default()),
            target: Some(nested_target(LOOPBACK_PORT)),
            registered: false,
        }
    } else {
        empty_nested()
    };
    restart_caddy(&bin, &cfg_path, listen.clone(), nested_placeholder.clone())?;
    let nested = if door.mode == "connect" {
        match register_nested(LOOPBACK_PORT) {
            Ok(n) => {
                if let Some(st) = live().lock().as_mut() {
                    st.nested = n.clone();
                }
                n
            }
            Err(e) => return Err(e),
        }
    } else {
        nested_placeholder
    };

    let mut st = status()?;
    st.applied = true;
    st.listen = listen;
    st.nested = nested;
    st.error = None;
    if let Some(h) = direct_hint {
        st.hint = Some(h);
    } else if door.mode == "connect" && !st.nested.registered {
        st.hint = Some(NO_PUBLIC_NESTED_HINT.to_string());
    }
    Ok(st)
}

/// True when the operator has persisted a front-door row (boot apply).
pub fn has_stored_front_door() -> bool {
    skin::front_door_is_stored()
}

/// Best-effort boot apply. Never panics; missing Caddy is logged.
pub fn maybe_apply_on_boot(daemon_port: u16) {
    if !has_stored_front_door() {
        return;
    }
    match apply(daemon_port) {
        Ok(st) => crate::log_debug!(
            "[skin-door] applied mode={} listen={} caddy.running={}",
            st.mode,
            st.listen,
            st.caddy.running
        ),
        Err(e) => crate::log_debug!("[skin-door] boot apply skipped: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("HOME");
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp =
            std::env::temp_dir().join(format!("k2-skin-door-{}-{}", std::process::id(), nanos));
        std::fs::create_dir_all(&tmp).expect("temp HOME");
        std::env::set_var("HOME", &tmp);
        f();
        match prev {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    fn spec(ui: Option<u16>, extra: Option<&str>) -> CaddyfileSpec {
        CaddyfileSpec {
            daemon_port: 60710,
            loopback_port: LOOPBACK_PORT,
            extra_listen: extra.map(|s| s.to_string()),
            ui_port: ui,
        }
    }

    #[test]
    fn render_allows_thread_and_overlay_not_grid() {
        let file = render_caddyfile(&spec(None, None));
        assert!(file.contains("/cli/thread"), "{file}");
        assert!(file.contains("/cli/overlay/events"), "{file}");
        assert!(file.contains("/boot-status"), "{file}");
        assert!(
            file.contains(PATH_FILTER_ERROR),
            "catch-all 403 JSON missing: {file}"
        );
        assert!(file.contains("403"), "{file}");
        assert!(file.contains("admin off"), "{file}");
        assert!(
            file.contains(&format!("127.0.0.1:{LOOPBACK_PORT}")),
            "{file}"
        );
        assert!(
            !file.contains(":38471"),
            "must not use air-gap port: {file}"
        );
        let lowered = file.to_ascii_lowercase();
        assert!(
            !lowered.contains("reverse_proxy") || !file.contains("/cli/sessions/grid"),
            "must not reverse_proxy grid: {file}"
        );
        assert!(!file.contains("/cli/sessions/grid"), "{file}");
        assert!(!file.contains("/cli/auth/login"), "{file}");
        assert!(!file.contains("/v1"), "{file}");
        assert!(!file.contains("/cli/sessions/bytes"), "{file}");
        assert!(!file.contains("/cli/terminal/"), "{file}");
        for line in file.lines() {
            if line.contains("reverse_proxy") {
                assert!(
                    !line.contains("/cli/sessions/grid")
                        && !line.contains("/cli/auth/login")
                        && !line.contains("/v1"),
                    "forbidden reverse_proxy line: {line}"
                );
            }
        }
    }

    #[test]
    fn render_ui_port_is_exact_root_not_whole_daemon() {
        let file = render_caddyfile(&spec(Some(5173), None));
        assert!(file.contains("127.0.0.1:5173"), "{file}");
        assert!(file.contains("@skinUi"), "{file}");
        assert!(file.contains(PATH_FILTER_ERROR), "{file}");
        assert!(!file.contains("/cli/sessions/grid"), "{file}");
    }

    #[test]
    fn render_direct_443_and_fallback_listen() {
        let with_443 = render_caddyfile(&spec(None, Some(":443")));
        assert!(with_443.contains("http://:443"), "{with_443}");
        assert!(
            with_443.contains(&format!("127.0.0.1:{LOOPBACK_PORT}")),
            "{with_443}"
        );
        let fallback = render_caddyfile(&spec(None, Some("0.0.0.0:38472")));
        assert!(fallback.contains("http://0.0.0.0:38472"), "{fallback}");
        assert!(
            !fallback.contains("http://127.0.0.1:38472, http://0.0.0.0:38472"),
            "same port twice would fail to bind: {fallback}"
        );
    }

    #[test]
    fn apply_without_caddy_is_caddy_missing() {
        with_temp_home(|| {
            let prev_path = std::env::var_os("PATH");
            std::env::set_var("PATH", "");
            let err = apply(60710).unwrap_err();
            match prev_path {
                Some(p) => std::env::set_var("PATH", p),
                None => std::env::remove_var("PATH"),
            }
            assert!(err.contains("caddy_missing"), "{err}");
            assert!(
                err.contains("brew install caddy") || err.contains("apt install"),
                "{err}"
            );
        });
    }

    #[test]
    fn status_default_is_connect_not_applied() {
        with_temp_home(|| {
            let st = status().expect("status");
            assert_eq!(st.mode, "connect");
            assert_eq!(st.connect_url, CONNECT_URL_STUB);
            assert_eq!(st.listen, format!("127.0.0.1:{LOOPBACK_PORT}"));
            assert!(!st.applied);
            assert_eq!(st.nested.label, "skin");
            assert!(!st.nested.registered);
            assert!(st.ui_port.is_none());
        });
    }
}
