//! Skin OIDC issuer sidecar (prd-skin-oidc-hydra-v1 leftover 123).
//!
//! Linux bake/supervise like mail: PATH `hydra` child, loopback only.
//! Mac: `supported=false`, toggle no-ops loud. Enable skins ≠ start Hydra.
//!
//! Ports (loopback): **4444 public** / **4445 admin**. Caddy Host table for
//! public OIDC on :443 is a follow-up. Hydra stores **no users/passwords**;
//! `subject` = K2 skin principal id. Login/consent SPA is deferred.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::json;

/// Loopback OIDC public port.
pub const PUBLIC_PORT: u16 = 4444;
/// Loopback Hydra admin port.
pub const ADMIN_PORT: u16 = 4445;

pub const PUBLIC_URL: &str = "http://127.0.0.1:4444/";
pub const ADMIN_URL: &str = "http://127.0.0.1:4445/";

pub const LINUX_ONLY_HINT: &str =
    "THIS FEATURE ONLY WORKS ON LINUX DEPLOYMENTS, THIS PAGE IS JUST HERE FOR EXAMPLE PURPOSES.";

pub const HYDRA_MISSING: &str = "hydra_missing: the OIDC issuer needs `hydra` on PATH. \
Install: sudo apt install ory (or download Ory Hydra from GitHub releases). \
The daemon does not apt-get. Enabling skins does not start Hydra.";

const ENABLED_HINT: &str = "OIDC public http://127.0.0.1:4444/ (loopback). \
Admin http://127.0.0.1:4445/. Subject = skin principal id; no users in Hydra. \
Caddy Host table for public OIDC on :443 is a follow-up.";

const OFF_HINT: &str =
    "Off. Enabling skins does not start Hydra. Subject = skin principal id; no users in Hydra.";

const ENABLED_NOT_RUNNING: &str = "Hydra is enabled in catalog but not running. POST {\"enabled\":true,\"apply\":true} to start (Linux, hydra on PATH).";

/// Runtime capability gate. Compiles everywhere; Mac reports unsupported.
pub fn hydra_supported() -> bool {
    if let Some(v) = fake_supported() {
        return v;
    }
    cfg!(target_os = "linux")
}

struct LiveHydra {
    child: Child,
    pid: u32,
}

fn live() -> &'static Mutex<Option<LiveHydra>> {
    static LIVE: OnceLock<Mutex<Option<LiveHydra>>> = OnceLock::new();
    LIVE.get_or_init(|| Mutex::new(None))
}

fn fake_running_flag() -> &'static AtomicBool {
    static F: AtomicBool = AtomicBool::new(false);
    &F
}

#[cfg(test)]
thread_local! {
    static FAKE_SUPPORTED: std::cell::RefCell<Option<bool>> = const { std::cell::RefCell::new(None) };
    static FAKE_BIN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FAKE_SPAWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn fake_supported() -> Option<bool> {
    #[cfg(test)]
    {
        return FAKE_SUPPORTED.with(|c| *c.borrow());
    }
    #[cfg(not(test))]
    None
}

fn fake_bin() -> bool {
    #[cfg(test)]
    {
        return FAKE_BIN.with(|c| c.get());
    }
    #[cfg(not(test))]
    false
}

fn fake_spawn() -> bool {
    #[cfg(test)]
    {
        return FAKE_SPAWN.with(|c| c.get());
    }
    #[cfg(not(test))]
    false
}

/// Serializes tests that touch the singleton `skin_hydra` row / live child.
#[cfg(test)]
pub(crate) fn hydra_test_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex as StdMutex, OnceLock};
    static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| StdMutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Linux-supervisor Fake: supported + PATH hydra without spawning a process.
#[cfg(test)]
pub fn with_fake_linux<R>(bin_present: bool, f: impl FnOnce() -> R) -> R {
    FAKE_SUPPORTED.with(|c| *c.borrow_mut() = Some(true));
    FAKE_BIN.with(|c| c.set(bin_present));
    FAKE_SPAWN.with(|c| c.set(true));
    fake_running_flag().store(false, Ordering::SeqCst);
    let r = f();
    stop();
    FAKE_SUPPORTED.with(|c| *c.borrow_mut() = None);
    FAKE_BIN.with(|c| c.set(false));
    FAKE_SPAWN.with(|c| c.set(false));
    fake_running_flag().store(false, Ordering::SeqCst);
    r
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn is_enabled() -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT enabled FROM skin_hydra WHERE id = 1", [], |r| {
        r.get::<_, i64>(0)
    })
    .ok()
    .map(|v| v != 0)
    .unwrap_or(false)
}

fn set_enabled(on: bool) -> Result<(), String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO skin_hydra (id, enabled, updated_at) VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET enabled = ?1, updated_at = ?2",
        rusqlite::params![if on { 1 } else { 0 }, now_secs()],
    )
    .map_err(|e| format!("skin_hydra upsert: {e}"))?;
    Ok(())
}

fn hydra_dir() -> PathBuf {
    k2_core::skin_door::skin_dir()
}

fn config_path() -> PathBuf {
    hydra_dir().join("hydra.yml")
}

fn pid_path() -> PathBuf {
    hydra_dir().join("hydra.pid")
}

fn log_path() -> PathBuf {
    hydra_dir().join("hydra.log")
}

fn ensure_dir() -> Result<PathBuf, String> {
    let dir = hydra_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

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

/// PATH-only. Do not apt-get; do not bundle Hydra.
pub fn resolve_hydra() -> Option<PathBuf> {
    if fake_bin() {
        return Some(PathBuf::from("/fake/hydra"));
    }
    if fake_spawn() && !fake_bin() {
        return None;
    }
    which_in_path("hydra")
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
}

fn read_pid_file() -> Option<u32> {
    let raw = std::fs::read_to_string(pid_path()).ok()?;
    raw.trim().parse().ok()
}

fn write_pid_file(pid: u32) {
    let path = pid_path();
    let _ = std::fs::write(&path, format!("{pid}\n"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
}

fn existing_system_secret(yml: &str) -> Option<String> {
    for line in yml.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("- ") {
            let s = rest.trim().trim_matches('"').trim_matches('\'');
            if s.len() >= 16 && s.chars().all(|c| c.is_ascii_hexdigit()) {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn system_secret() -> String {
    if let Ok(raw) = std::fs::read_to_string(config_path()) {
        if let Some(s) = existing_system_secret(&raw) {
            return s;
        }
    }
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Hydra YAML: loopback only, memory DSN, no users. `subject` = skin principal id.
fn hydra_yml(secret: &str) -> String {
    format!(
        r#"# Ory Hydra sidecar for this K2 box (loopback only).
# subject = K2 skin principal id (SQLite). Hydra stores no users or passwords.
# Issuer URL placeholder: {PUBLIC_URL} — public OIDC on :443 is a Caddy Host follow-up.
# Login/consent app is deferred (not this slice).
#
# Ports: {PUBLIC_PORT} public / {ADMIN_PORT} admin — 127.0.0.1 only.
dsn: memory
log:
  level: info
serve:
  public:
    host: 127.0.0.1
    port: {PUBLIC_PORT}
  admin:
    host: 127.0.0.1
    port: {ADMIN_PORT}
urls:
  self:
    issuer: {PUBLIC_URL}
  login: http://127.0.0.1:4455/login
  consent: http://127.0.0.1:4455/consent
  logout: http://127.0.0.1:4455/logout
secrets:
  system:
    - {secret}
"#
    )
}

fn write_config() -> Result<PathBuf, String> {
    ensure_dir()?;
    let path = config_path();
    let yml = hydra_yml(&system_secret());
    std::fs::write(&path, yml).map_err(|e| format!("write {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

fn spawn_hydra(bin: &Path, cfg: &Path) -> Result<Child, String> {
    let log = log_path();
    let mut cmd = Command::new(bin);
    cmd.arg("serve")
        .arg("all")
        .arg("--config")
        .arg(cfg)
        .arg("--sqa-opt-out")
        .arg("--dev")
        .stdin(Stdio::null());
    match std::fs::File::create(&log) {
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
    cmd.spawn().map_err(|e| format!("spawn hydra: {e}"))
}

pub fn stop() {
    fake_running_flag().store(false, Ordering::SeqCst);
    if let Some(mut live_proc) = live().lock().take() {
        let _ = live_proc.child.kill();
        let _ = live_proc.child.wait();
        kill_pid(live_proc.pid);
    }
    if let Some(pid) = read_pid_file() {
        kill_pid(pid);
    }
    let _ = std::fs::remove_file(pid_path());
}

fn start() -> Result<(), String> {
    if !hydra_supported() {
        stop();
        return Ok(());
    }
    if fake_spawn() {
        if resolve_hydra().is_none() {
            stop();
            return Ok(());
        }
        fake_running_flag().store(true, Ordering::SeqCst);
        return Ok(());
    }
    let Some(bin) = resolve_hydra() else {
        stop();
        return Ok(());
    };
    let cfg = write_config()?;
    stop();
    let mut child = spawn_hydra(&bin, &cfg)?;
    let pid = child.id();
    write_pid_file(pid);
    std::thread::sleep(Duration::from_millis(120));
    match child.try_wait() {
        Ok(Some(status)) => {
            let log = std::fs::read_to_string(log_path()).unwrap_or_default();
            let tail: String = log
                .lines()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" | ");
            return Err(format!("hydra exited immediately ({status}). {tail}"));
        }
        Ok(None) => {}
        Err(e) => return Err(format!("hydra wait: {e}")),
    }
    *live().lock() = Some(LiveHydra { child, pid });
    Ok(())
}

pub fn is_running() -> bool {
    if fake_running_flag().load(Ordering::SeqCst) {
        return true;
    }
    {
        let mut g = live().lock();
        if let Some(live_proc) = g.as_mut() {
            match live_proc.child.try_wait() {
                Ok(None) => return true,
                Ok(Some(_)) => {
                    g.take();
                }
                Err(_) => {}
            }
        }
    }
    if let Some(pid) = read_pid_file() {
        return pid_alive(pid);
    }
    false
}

fn hint(supported: bool, enabled: bool, running: bool, missing: bool) -> String {
    if !supported {
        return LINUX_ONLY_HINT.to_string();
    }
    if missing {
        return HYDRA_MISSING.to_string();
    }
    if running {
        return ENABLED_HINT.to_string();
    }
    if enabled {
        return ENABLED_NOT_RUNNING.to_string();
    }
    OFF_HINT.to_string()
}

/// Snapshot for GET `/cli/skin/hydra`. Never starts Hydra.
pub fn status_json() -> serde_json::Value {
    let supported = hydra_supported();
    let enabled = is_enabled();
    let running = supported && is_running();
    let missing = supported && resolve_hydra().is_none();
    json!({
        "supported": supported,
        "enabled": enabled,
        "running": running,
        "publicUrl": PUBLIC_URL,
        "adminUrl": ADMIN_URL,
        "hint": hint(supported, enabled, running, missing),
    })
}

/// Persist `enabled`. When `apply`, start/stop the supervisor on Linux.
/// Mac: persist + loud no-op (never running). Missing binary: hydra_missing, no apt-get.
pub fn apply(enabled: bool, do_apply: bool) -> Result<serde_json::Value, String> {
    set_enabled(enabled)?;
    if do_apply {
        if enabled {
            start()?;
        } else {
            stop();
        }
    }
    Ok(status_json())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset_row() {
        k2_core::db::init_for_tests();
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute("DELETE FROM skin_hydra", []);
    }

    #[test]
    fn supported_matches_target_os_without_fake() {
        let _g = hydra_test_lock();
        assert_eq!(hydra_supported(), cfg!(target_os = "linux"));
    }

    #[test]
    fn get_unsupported_on_non_linux() {
        let _g = hydra_test_lock();
        reset_row();
        stop();
        if cfg!(target_os = "linux") {
            return;
        }
        let v = status_json();
        assert_eq!(v["supported"], false, "{v}");
        assert_eq!(v["enabled"], false, "{v}");
        assert_eq!(v["running"], false, "{v}");
        assert_eq!(v["publicUrl"], PUBLIC_URL);
        assert_eq!(v["adminUrl"], ADMIN_URL);
        let hint = v["hint"].as_str().unwrap_or("");
        assert!(hint.contains("LINUX"), "{hint}");
    }

    #[test]
    fn toggle_off_is_not_running() {
        let _g = hydra_test_lock();
        reset_row();
        let v = apply(false, true).expect("apply off");
        assert_eq!(v["enabled"], false, "{v}");
        assert_eq!(v["running"], false, "{v}");
        assert!(!is_running());
    }

    #[test]
    fn persist_enabled_does_not_start_without_apply() {
        let _g = hydra_test_lock();
        reset_row();
        with_fake_linux(true, || {
            let v = apply(true, false).expect("persist");
            assert_eq!(v["enabled"], true, "{v}");
            assert_eq!(v["running"], false, "{v}");
            assert_eq!(v["supported"], true, "{v}");
        });
    }

    #[test]
    fn fake_linux_start_stop() {
        let _g = hydra_test_lock();
        reset_row();
        with_fake_linux(true, || {
            let on = apply(true, true).expect("start");
            assert_eq!(on["supported"], true, "{on}");
            assert_eq!(on["enabled"], true, "{on}");
            assert_eq!(on["running"], true, "{on}");
            assert!(!on["hint"].as_str().unwrap_or("").contains("hydra_missing"));
            let off = apply(false, true).expect("stop");
            assert_eq!(off["enabled"], false, "{off}");
            assert_eq!(off["running"], false, "{off}");
            assert!(!is_running());
        });
    }

    #[test]
    fn fake_linux_missing_binary_is_hydra_missing_not_running() {
        let _g = hydra_test_lock();
        reset_row();
        with_fake_linux(false, || {
            let v = apply(true, true).expect("enable without bin");
            assert_eq!(v["enabled"], true, "{v}");
            assert_eq!(v["running"], false, "{v}");
            let hint = v["hint"].as_str().unwrap_or("");
            assert!(hint.contains("hydra_missing"), "{hint}");
            assert!(hint.contains("apt"), "{hint}");
            assert!(
                hint.to_ascii_lowercase().contains("does not apt-get"),
                "{hint}"
            );
        });
    }

    #[test]
    fn yml_comments_subject_and_issuer_placeholder() {
        let yml = hydra_yml("aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899");
        assert!(yml.contains("subject = K2 skin principal id"), "{yml}");
        assert!(yml.contains("no users"), "{yml}");
        assert!(yml.contains(PUBLIC_URL), "{yml}");
        assert!(yml.contains("127.0.0.1"), "{yml}");
        assert!(yml.contains(&PUBLIC_PORT.to_string()), "{yml}");
        assert!(yml.contains(&ADMIN_PORT.to_string()), "{yml}");
        assert!(yml.contains("\n  public:\n"), "{yml}");
        assert!(yml.contains("\n    host: 127.0.0.1\n"), "{yml}");
        assert!(!yml.contains("password:"), "{yml}");
        assert!(yml.contains("no users or passwords"), "{yml}");
    }
}
