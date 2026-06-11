//! TunnelConnector — launches and supervises the `frpc` child process
//! that dials the K2 Connect frps server.
//!
//! Lifecycle:
//!   * `start()` resolves the `frpc` binary, renders the config TOML to a
//!     0600 file under `~/.k2so/`, spawns `frpc -c <file>`, and starts a
//!     supervisor thread that captures stdout/stderr to a log and
//!     restarts the child on unexpected exit with exponential backoff.
//!   * `stop()` flips the desired-state flag and signals the child to
//!     terminate; the supervisor observes the flag and does NOT restart.
//!   * `status()` reports running/stopped + the predicted public URL.
//!
//! The connector is a process-wide singleton (one tunnel per daemon),
//! held behind a `Mutex` in [`STATE`]. The binary path is pluggable
//! ([`FrpcBinary`]) so tests can inject a fake and production can locate
//! `frpc` via PATH or common install dirs.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;

use super::config::{self, TunnelConfig, SUBDOMAIN_HOST};
use super::render::render_frpc_toml;

/// Where to find the `frpc` binary.
#[derive(Debug, Clone)]
pub enum FrpcBinary {
    /// Locate via PATH, then a list of common install locations.
    Auto,
    /// An explicit, caller-supplied path (config override / tests).
    Explicit(PathBuf),
}

impl Default for FrpcBinary {
    fn default() -> Self {
        FrpcBinary::Auto
    }
}

/// Common non-PATH locations to probe for a `frpc` install.
fn common_frpc_locations() -> Vec<PathBuf> {
    let mut v = vec![
        PathBuf::from("/opt/homebrew/bin/frpc"),
        PathBuf::from("/usr/local/bin/frpc"),
        PathBuf::from("/usr/bin/frpc"),
    ];
    if let Some(home) = dirs::home_dir() {
        v.push(home.join(".local/bin/frpc"));
        v.push(home.join(".k2so/bin/frpc"));
    }
    v
}

/// Resolve the `frpc` executable, or a clear "not installed" error.
/// Does NOT auto-download — surfacing the requirement is intentional.
pub fn resolve_frpc(bin: &FrpcBinary) -> Result<PathBuf, String> {
    match bin {
        FrpcBinary::Explicit(p) => {
            if p.exists() {
                Ok(p.clone())
            } else {
                Err(format!("frpc not found at configured path: {}", p.display()))
            }
        }
        FrpcBinary::Auto => {
            // 1) PATH lookup via `which`-style probing of PATH dirs.
            if let Some(found) = which_in_path("frpc") {
                return Ok(found);
            }
            // 2) Common install dirs.
            for cand in common_frpc_locations() {
                if cand.exists() {
                    return Ok(cand);
                }
            }
            Err(
                "frpc not installed: the K2 Connect tunnel requires the `frpc` \
                 client binary (fatedier/frp v0.61+). Install it via your package \
                 manager (e.g. `brew install frpc`) or download a release from \
                 https://github.com/fatedier/frp/releases and place it on your PATH."
                    .to_string(),
            )
        }
    }
}

/// Minimal PATH lookup (no external `which` dep). Returns the first
/// executable `name` found in the `$PATH` directories.
fn which_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if is_executable(&cand) {
            return Some(cand);
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

/// Path the rendered frpc config is written to.
fn frpc_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
        .join("frpc.toml")
}

/// Path the frpc child's stdout/stderr is captured to.
pub fn frpc_log_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
        .join("frpc.log")
}

/// Live connector state — the supervised child + the desired-state flag.
struct ConnectorState {
    /// The currently-running config (resolved local_port).
    cfg: TunnelConfig,
    resolved_local_port: u16,
    /// The frpc child handle. `None` between restarts.
    child: Arc<Mutex<Option<Child>>>,
    /// Desired state: `true` = should be running (supervisor restarts on
    /// exit); `false` = stop requested (supervisor must not restart).
    running: Arc<AtomicBool>,
}

static STATE: OnceLock<Mutex<Option<ConnectorState>>> = OnceLock::new();

fn state() -> &'static Mutex<Option<ConnectorState>> {
    STATE.get_or_init(|| Mutex::new(None))
}

/// Reported status of the connector.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TunnelStatus {
    pub running: bool,
    /// Predicted public URL `https://<subdomain>.k2.dev` (the server may
    /// canonicalize the label; this is the requested value).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdomain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_addr: Option<String>,
    /// Whether an `frpc` binary can be resolved (PATH + common install
    /// dirs). Computed with the SAME [`resolve_frpc`] the connector uses
    /// to launch, so the UI's "frpc not installed" hint can never
    /// disagree with whether a tunnel can actually start. Always emitted
    /// (no skip) so the client never sees `undefined` and mis-renders the
    /// warning.
    pub frpc_installed: bool,
}

impl TunnelStatus {
    fn stopped() -> Self {
        Self {
            running: false,
            public_url: None,
            subdomain: None,
            local_port: None,
            server_addr: None,
            frpc_installed: resolve_frpc(&FrpcBinary::Auto).is_ok(),
        }
    }
}

/// Start the tunnel.
///
/// * `subdomain_override` — when `Some`, supersedes the stored config's
///   subdomain (and is persisted back).
/// * `default_local_port` — the live daemon port, used when the config
///   doesn't pin a `local_port`.
/// * `bin` — `frpc` binary resolution strategy.
///
/// Errors loudly (no silent fallback) when: no token configured, frpc is
/// missing, the config can't be written, or spawn fails. Idempotent: if
/// already running, returns the current status without respawning.
pub fn start(
    subdomain_override: Option<String>,
    default_local_port: u16,
    bin: &FrpcBinary,
) -> Result<TunnelStatus, String> {
    let mut guard = state().lock().unwrap_or_else(|p| p.into_inner());

    // Idempotent: already supervising a live child.
    if let Some(st) = guard.as_ref() {
        if st.running.load(Ordering::SeqCst) {
            return Ok(status_from(st));
        }
    }

    // Load + reconcile config.
    let mut cfg = config::load()?;
    if let Some(sub) = subdomain_override {
        cfg.subdomain = sub;
    }
    if !cfg.is_connectable() {
        return Err(
            "tunnel not configured: set a K2SO bearer token first \
             (no token in ~/.k2so/tunnel.json)"
                .to_string(),
        );
    }
    // The K2SO tunnel ALWAYS exposes the live daemon, whose HTTP port is
    // ephemeral and ROTATES on every daemon restart (app update, reboot).
    // So we MUST forward to the live `default_local_port` and must NEVER
    // persist a pinned `local_port`: a pinned snapshot goes stale the moment
    // the daemon restarts on a new port, leaving frpc forwarding to a dead
    // socket and the host silently unreachable — i.e. **every software
    // update would lose the user's remote access**. Always resolve live and
    // keep the stored config port-less so future starts re-resolve.
    let resolved_local_port = default_local_port;
    let mut to_save = cfg.clone();
    to_save.local_port = None;
    config::save(&to_save)?;

    // Resolve frpc + render config to disk (0600).
    let frpc = resolve_frpc(bin)?;
    let toml = render_frpc_toml(&cfg, resolved_local_port);
    write_config_file(&toml)?;

    // Reap any STRAY frpc bound to our config before spawning a fresh one.
    // This is the load-bearing self-heal for the multi-frpc failure mode:
    // when the daemon exits WITHOUT cleanly killing frpc (SIGKILL, panic,
    // OS shutdown that races the supervisor — none of which run a Drop or
    // shutdown hook), the child is orphaned (reparented to init) but keeps
    // its frps proxy registration alive, still forwarding to the now-dead
    // OLD daemon port. On the next boot `start()`'s idempotency guard above
    // is empty (fresh process), so without this reap we'd spawn a SECOND
    // frpc while the orphan still owns the `k2so-<sub>` proxy name (frps
    // keeps the first registrant) — the orphan serves EOFs and the new
    // client is rejected with "proxy already exists", silently breaking
    // remote access. We reach here only when no live child is tracked
    // (guard returned early otherwise), so every match is genuinely stale.
    reap_stray_frpc(&frpc_config_path());

    // Spawn + supervise.
    let child = Arc::new(Mutex::new(None));
    let running = Arc::new(AtomicBool::new(true));
    spawn_supervised(frpc, child.clone(), running.clone())?;

    // K2SO #674: while the tunnel is up, the DAEMON renews the subdomain
    // lease on its own timer so it never lapses with the Settings panel
    // closed or the daemon running headless. Tied to this start: the
    // renewal thread watches the SAME `running` flag the supervisor does
    // and self-exits the moment `stop()` flips it false.
    spawn_lease_renewal(&cfg, running.clone());

    let st = ConnectorState {
        cfg,
        resolved_local_port,
        child,
        running,
    };
    let status = status_from(&st);
    *guard = Some(st);
    Ok(status)
}

/// Stop the tunnel. Flips desired-state to stopped (so the supervisor
/// won't restart) and kills the live child. Idempotent — stopping a
/// stopped tunnel is `Ok`.
pub fn stop() -> Result<(), String> {
    let mut guard = state().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(st) = guard.as_ref() {
        st.running.store(false, Ordering::SeqCst);
        if let Some(child) = st.child.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
            // Best-effort graceful kill. frpc has no special signal
            // protocol; SIGKILL via `kill()` is the portable stop.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    *guard = None;
    Ok(())
}

/// Current connector status.
pub fn status() -> TunnelStatus {
    let guard = state().lock().unwrap_or_else(|p| p.into_inner());
    match guard.as_ref() {
        Some(st) if st.running.load(Ordering::SeqCst) => status_from(st),
        _ => TunnelStatus::stopped(),
    }
}

fn status_from(st: &ConnectorState) -> TunnelStatus {
    let sub = st.cfg.subdomain.trim();
    let public_url = if sub.is_empty() {
        None
    } else {
        Some(format!("https://{sub}.{SUBDOMAIN_HOST}"))
    };
    TunnelStatus {
        running: st.running.load(Ordering::SeqCst),
        public_url,
        subdomain: (!sub.is_empty()).then(|| sub.to_string()),
        local_port: Some(st.resolved_local_port),
        server_addr: Some(st.cfg.server_addr.clone()),
        frpc_installed: resolve_frpc(&FrpcBinary::Auto).is_ok(),
    }
}

/// The `pkill -f` pattern that matches ONLY frpc processes launched with
/// our config file (`<frpc> -c <cfg>`), never an unrelated frpc the user
/// may run for their own tunnels. Kept separate so the match is unit-tested
/// without spawning real processes.
fn stray_frpc_pattern(cfg_path: &Path) -> String {
    format!("frpc -c {}", cfg_path.to_string_lossy())
}

/// Best-effort kill of stray frpc bound to our config (see call site in
/// `start()` for why this is required). Narrowly matched so we never touch
/// another tunnel. Errors are swallowed: a missing `pkill` or no-match is
/// the normal, healthy case.
#[cfg(unix)]
fn reap_stray_frpc(cfg_path: &Path) {
    let _ = Command::new("pkill")
        .arg("-f")
        .arg(stray_frpc_pattern(cfg_path))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(unix))]
fn reap_stray_frpc(_cfg_path: &Path) {}

/// Write the rendered TOML to `~/.k2so/frpc.toml` (0600) via tmp+rename.
fn write_config_file(toml: &str) -> Result<(), String> {
    let path = frpc_config_path();
    let dir = path
        .parent()
        .ok_or_else(|| "frpc config has no parent dir".to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let tmp = dir.join(format!("frpc.toml.tmp.{}", std::process::id()));
    std::fs::write(&tmp, toml.as_bytes()).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    restrict_mode(&tmp);
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("rename {}: {e}", path.display())
    })?;
    restrict_mode(&path);
    Ok(())
}

#[cfg(unix)]
fn restrict_mode(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_mode(_p: &Path) {}

/// Spawn the frpc child once and start the supervisor thread that
/// captures output and restarts on unexpected exit with backoff.
fn spawn_supervised(
    frpc: PathBuf,
    child_slot: Arc<Mutex<Option<Child>>>,
    running: Arc<AtomicBool>,
) -> Result<(), String> {
    // First spawn happens synchronously so `start()` fails loud if the
    // very first launch can't even exec.
    let first = spawn_once(&frpc)?;
    *child_slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(first);

    let frpc_thread = frpc.clone();
    std::thread::Builder::new()
        .name("k2so-frpc-supervisor".to_string())
        .spawn(move || {
            let mut backoff = Duration::from_millis(500);
            let max_backoff = Duration::from_secs(30);
            loop {
                // Take the current child to wait on it.
                let mut child = match child_slot
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .take()
                {
                    Some(c) => c,
                    None => {
                        // No child to wait on. If we've been told to
                        // stop, exit; otherwise spawn one.
                        if !running.load(Ordering::SeqCst) {
                            return;
                        }
                        match spawn_once(&frpc_thread) {
                            Ok(c) => c,
                            Err(e) => {
                                crate::log_debug!("[tunnel] respawn failed: {e}");
                                std::thread::sleep(backoff);
                                backoff = (backoff * 2).min(max_backoff);
                                continue;
                            }
                        }
                    }
                };

                let status = child.wait();
                if !running.load(Ordering::SeqCst) {
                    // Stop requested — do not restart.
                    return;
                }
                crate::log_debug!(
                    "[tunnel] frpc exited ({:?}); restarting in {:?}",
                    status.ok().and_then(|s| s.code()),
                    backoff
                );
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(max_backoff);
                if !running.load(Ordering::SeqCst) {
                    return;
                }
                match spawn_once(&frpc_thread) {
                    Ok(c) => {
                        *child_slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(c);
                    }
                    Err(e) => {
                        crate::log_debug!("[tunnel] respawn failed: {e}");
                    }
                }
            }
        })
        .map_err(|e| format!("spawn supervisor thread: {e}"))?;
    Ok(())
}

/// K2SO #674 — spawn the daemon-owned lease-renewal loop for a running
/// tunnel. The loop re-POSTs the `claim_subdomain` heartbeat every
/// [`lease::RENEW_INTERVAL`] so `<sub>.k2.dev` keeps routing to this
/// machine, with NO dependence on any client being connected or the
/// Settings panel being mounted (works fully headless).
///
/// Lifecycle is tied to the tunnel: the loop watches the same `running`
/// flag the frpc supervisor does and returns the moment [`stop`] flips it
/// false. The interval is split into short sleeps so a stop is observed
/// promptly rather than after a full minute.
///
/// No renewal target (no subdomain label or no client-persisted device id
/// in the config — e.g. a manual token-only config) → the loop logs once
/// and exits; the tunnel still runs, it just isn't lease-renewed here.
fn spawn_lease_renewal(cfg: &TunnelConfig, running: Arc<AtomicBool>) {
    let target = match super::lease::LeaseTarget::from_config(cfg) {
        Some(t) => t,
        None => {
            crate::log_debug!(
                "[tunnel/lease] no renewal target (no subdomain/device id in config) — \
                 skipping daemon-side lease renewal"
            );
            return;
        }
    };

    let spawned = std::thread::Builder::new()
        .name("k2so-tunnel-lease".to_string())
        .spawn(move || {
            crate::log_debug!(
                "[tunnel/lease] daemon-owned lease renewal started for {} (every {:?})",
                target.label,
                super::lease::RENEW_INTERVAL
            );
            // Heartbeat immediately on start so a fresh tunnel doesn't wait
            // a full interval for its first renewal (the renderer's
            // one-shot claim covers the very start, but an auto-start/
            // headless boot has no renderer claim at all).
            loop {
                if !running.load(Ordering::SeqCst) {
                    break;
                }
                match super::lease::renew_once(&target) {
                    Ok(true) => { /* lease held — quiet on the happy path */ }
                    Ok(false) => crate::log_debug!(
                        "[tunnel/lease] {} now held by another device — heartbeat not applied",
                        target.label
                    ),
                    Err(e) => crate::log_debug!(
                        "[tunnel/lease] renewal tick failed (will retry next interval): {e}"
                    ),
                }
                // Sleep the interval in short slices so `stop()` is observed
                // within ~1s rather than up to a full minute later.
                let mut remaining = super::lease::RENEW_INTERVAL;
                let slice = Duration::from_secs(1);
                while remaining > Duration::ZERO {
                    if !running.load(Ordering::SeqCst) {
                        break;
                    }
                    let nap = remaining.min(slice);
                    std::thread::sleep(nap);
                    remaining = remaining.saturating_sub(nap);
                }
            }
            crate::log_debug!("[tunnel/lease] lease renewal stopped for {}", target.label);
        });
    if let Err(e) = spawned {
        // A failure to spawn the renewal thread must not fail tunnel start
        // — the tunnel still works, it just won't be lease-renewed by the
        // daemon. Log loudly so the regression is visible.
        crate::log_debug!("[tunnel/lease] WARN: failed to spawn lease renewal thread: {e}");
    }
}

/// Spawn a single `frpc -c <config>` child, redirecting stdout+stderr
/// into the append-mode log. Returns the child handle.
fn spawn_once(frpc: &Path) -> Result<Child, String> {
    let cfg_path = frpc_config_path();
    let log = open_log()?;
    let log_err = log
        .try_clone()
        .map_err(|e| format!("clone log handle: {e}"))?;
    let mut child = Command::new(frpc)
        .arg("-c")
        .arg(&cfg_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn frpc ({}): {e}", frpc.display()))?;

    // Pump stdout/stderr to the log on detached threads so the pipes
    // never fill and block the child. We do NOT log the rendered config
    // or token — only frpc's own output (which never echoes the meta).
    if let Some(out) = child.stdout.take() {
        pump(out, log);
    }
    if let Some(err) = child.stderr.take() {
        pump(err, log_err);
    }
    Ok(child)
}

fn pump(reader: impl std::io::Read + Send + 'static, mut sink: std::fs::File) {
    use std::io::Write;
    std::thread::spawn(move || {
        let buf = BufReader::new(reader);
        for line in buf.lines() {
            match line {
                Ok(l) => {
                    let _ = writeln!(sink, "{l}");
                }
                Err(_) => break,
            }
        }
    });
}

fn open_log() -> Result<std::fs::File, String> {
    let path = frpc_log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("open frpc log {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::test_support::with_temp_home;

    #[test]
    fn stray_frpc_pattern_matches_only_our_config() {
        // The reap must target frpc launched with OUR config path and
        // nothing else — a bare `frpc` pattern would nuke an unrelated
        // tunnel the user runs. Pin the exact `pkill -f` string.
        let pat = stray_frpc_pattern(Path::new("/Users/x/.k2so/frpc.toml"));
        assert_eq!(pat, "frpc -c /Users/x/.k2so/frpc.toml");
        // Must carry the config path (so it can't match an arbitrary frpc).
        assert!(pat.contains("/.k2so/frpc.toml"));
        // Must be scoped by `-c <cfg>`, not a bare process name.
        assert!(pat.starts_with("frpc -c "));
    }

    #[test]
    fn resolve_frpc_explicit_missing_errors_clearly() {
        let err = resolve_frpc(&FrpcBinary::Explicit(PathBuf::from(
            "/definitely/not/here/frpc",
        )))
        .unwrap_err();
        assert!(
            err.contains("frpc not found at configured path"),
            "expected configured-path error, got: {err}"
        );
    }

    #[test]
    fn resolve_frpc_auto_missing_surfaces_install_hint() {
        // Point PATH at an empty dir and HOME at a tempdir so none of the
        // common locations resolve — we must get the install guidance,
        // not a silent success.
        with_temp_home(|| {
            let empty = std::env::temp_dir().join(format!("k2so-empty-{}", std::process::id()));
            std::fs::create_dir_all(&empty).expect("mk empty dir");
            let prev = std::env::var_os("PATH");
            std::env::set_var("PATH", &empty);
            let res = resolve_frpc(&FrpcBinary::Auto);
            match prev {
                Some(p) => std::env::set_var("PATH", p),
                None => std::env::remove_var("PATH"),
            }
            let err = res.expect_err("frpc must be unresolvable with empty PATH + temp HOME");
            assert!(
                err.contains("frpc not installed"),
                "expected install hint, got: {err}"
            );
            assert!(
                err.contains("fatedier/frp"),
                "install hint should point at the frp project, got: {err}"
            );
        });
    }

    #[test]
    fn resolve_frpc_finds_executable_on_path() {
        with_temp_home(|| {
            let bin_dir = std::env::temp_dir().join(format!("k2so-bin-{}", std::process::id()));
            std::fs::create_dir_all(&bin_dir).expect("mk bin dir");
            let fake = bin_dir.join("frpc");
            std::fs::write(&fake, "#!/bin/sh\nexit 0\n").expect("write fake frpc");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))
                    .expect("chmod fake frpc");
            }
            let prev = std::env::var_os("PATH");
            std::env::set_var("PATH", &bin_dir);
            let res = resolve_frpc(&FrpcBinary::Auto);
            match prev {
                Some(p) => std::env::set_var("PATH", p),
                None => std::env::remove_var("PATH"),
            }
            assert_eq!(res.expect("should find fake frpc on PATH"), fake);
        });
    }

    #[test]
    fn start_without_token_errors_and_does_not_spawn() {
        with_temp_home(|| {
            // Fresh config (no token).
            let res = start(None, 57839, &FrpcBinary::Explicit(PathBuf::from("/bin/true")));
            let err = res.expect_err("start must refuse without a token");
            assert!(
                err.contains("tunnel not configured"),
                "expected not-configured error, got: {err}"
            );
            assert!(!status().running, "no child should be running after failed start");
        });
    }

    #[test]
    fn start_with_missing_frpc_surfaces_install_error() {
        with_temp_home(|| {
            config::save(&TunnelConfig {
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                ..Default::default()
            })
            .expect("seed config");
            let err = start(
                None,
                57839,
                &FrpcBinary::Explicit(PathBuf::from("/no/such/frpc")),
            )
            .expect_err("missing frpc must fail start");
            assert!(err.contains("frpc not found"), "got: {err}");
            assert!(!status().running);
        });
    }

    #[test]
    fn status_is_stopped_before_any_start() {
        with_temp_home(|| {
            // Ensure a clean singleton for this test.
            let _ = stop();
            assert_eq!(status(), TunnelStatus::stopped());
        });
    }

    #[test]
    fn stop_is_idempotent_on_stopped_connector() {
        with_temp_home(|| {
            stop().expect("first stop ok");
            stop().expect("second stop ok");
            assert!(!status().running);
        });
    }

    /// Dumps the rendered frpc TOML for the spec example so a human can
    /// eyeball it (`cargo test -p k2so-core dump_spec -- --ignored
    /// --nocapture`). Token is a placeholder. NOT a real-network test.
    #[test]
    #[ignore = "diagnostic: prints the spec-example frpc TOML"]
    fn dump_spec_example_toml() {
        let cfg = TunnelConfig {
            token: "REDACTED".to_string(),
            subdomain: "rosson".to_string(),
            local_port: Some(57839),
            ..Default::default()
        };
        println!("{}", render_frpc_toml(&cfg, 57839));
    }

    /// REAL-PROCESS, REAL-NETWORK test — gated `#[ignore]` so `cargo
    /// test` never spawns a live frpc against the production frps box
    /// (which would collide with the parent's live validation). Run
    /// manually only, with a real token in ~/.k2so/tunnel.json and frpc
    /// installed.
    #[test]
    #[ignore = "spawns a real frpc against the live K2 Connect server"]
    fn live_start_stop_roundtrip() {
        let st = start(Some("rosson".to_string()), 57839, &FrpcBinary::Auto)
            .expect("start live tunnel");
        assert!(st.running);
        std::thread::sleep(Duration::from_secs(2));
        assert!(status().running);
        stop().expect("stop live tunnel");
        assert!(!status().running);
    }
}
