//! TunnelConnector — launches and supervises the `frpc` child process
//! that dials the K2 Connect frps server.
//!
//! Lifecycle:
//!   * `start()` resolves the `frpc` binary, renders the config TOML to a
//!     0600 file under `~/.k2/`, spawns `frpc -c <file>`, and starts a
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
use std::time::{Duration, Instant};

use serde::Serialize;

use super::config::{self, RelayEndpoint, TunnelConfig, SUBDOMAIN_HOST};
use super::failover::{RelaySelector, RelaySwitch};
use super::render::render_frpc_toml_for_relay;

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
        v.push(home.join(".k2/bin/frpc"));
        // Legacy location pre-`.k2so`→`.k2` cutover (still a valid candidate
        // via the ~/.k2so→~/.k2 compat symlink, but listed explicitly).
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

/// A child that stayed up at least this long counts as a SUCCESSFUL
/// relay connection for failover accounting: frpc exits fast on a failed
/// login / unreachable frps (`loginFailExit` defaults true), so a
/// quick death means "this relay isn't answering" while a long run means
/// the dial worked (even if the process later died for another reason).
const HEALTHY_UPTIME: Duration = Duration::from_secs(30);

/// Poll cadence while waiting on the child in multi-relay mode (the
/// single-relay path keeps the original blocking `wait()`). Matches the
/// 1 s slice the lease/subdomain loops use to observe `stop()` promptly.
const SUPERVISE_POLL: Duration = Duration::from_secs(1);

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
            "tunnel not configured: select one of your purchased subdomains in \
             Settings → K2 Connect first (picking it binds its access token; \
             no token in ~/.k2/tunnel.json)"
                .to_string(),
        );
    }
    // The K2 Connect tunnel ALWAYS exposes the live daemon, whose HTTP port is
    // ephemeral and ROTATES on every daemon restart (app update, reboot).
    // So we MUST forward to the live `default_local_port` and must NEVER
    // persist a pinned `local_port`: a pinned snapshot goes stale the moment
    // the daemon restarts on a new port, leaving frpc forwarding to a dead
    // socket and the host silently unreachable — i.e. **every software
    // update would lose the user's remote access**. Always resolve live and
    // keep the stored config port-less so future starts re-resolve.
    // E2E (PRD §4 Option A) — the DEFAULT path for ~everyone as of 0.40.6+:
    // frpc must forward the ENCRYPTED stream to the daemon's rustls HTTPS
    // listener, NOT to its cleartext HTTP port. Because E2E is now on by
    // default, a tunnel start MUST be able to bring the listener up on
    // demand — obtain/install the per-subdomain cert and bind the listener
    // BEFORE serving — rather than erroring when the port wasn't published
    // at boot (e.g. a brand-new subdomain configured after the daemon
    // started, when boot's `maybe_spawn` had no subdomain yet).
    // `ensure_https_port` does exactly that via the daemon-registered hook
    // (idempotent: returns the live port if the listener is already up).
    // It still fails LOUD on issuance/bind failure — we never silently
    // forward cleartext to the HTTP port, which would defeat the entire
    // "relay sees only ciphertext" guarantee.
    let e2e = config::e2e_enabled(&cfg);
    let resolved_local_port = if e2e {
        super::tls::ensure_https_port(default_local_port)?
    } else {
        default_local_port
    };
    let mut to_save = cfg.clone();
    to_save.local_port = None;
    config::save(&to_save)?;

    // Resolve frpc + render config to disk (0600). `e2e` selects the proxy
    // type (https vs http) and was used above to pick `resolved_local_port`.
    // Always dial the PREFERRED relay first (index 0 of the fallback list);
    // for a legacy single-endpoint config that IS `server_addr`/`server_port`,
    // so the rendered TOML is byte-identical to the pre-failover path.
    let frpc = resolve_frpc(bin)?;
    write_relay_config(&cfg, &cfg.relay_list()[0], resolved_local_port, e2e)?;

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

    // Spawn + supervise. The supervisor gets the config + render inputs so
    // it can rotate relays (re-render frpc.toml, respawn) on repeated
    // failure — see `spawn_supervised`.
    let child = Arc::new(Mutex::new(None));
    let running = Arc::new(AtomicBool::new(true));
    spawn_supervised(
        frpc,
        child.clone(),
        running.clone(),
        &cfg,
        resolved_local_port,
        e2e,
    )?;

    // K2SO #674: while the tunnel is up, the DAEMON renews the subdomain
    // lease on its own timer so it never lapses with the Settings panel
    // closed or the daemon running headless. Tied to this start: the
    // renewal thread watches the SAME `running` flag the supervisor does
    // and self-exits the moment `stop()` flips it false.
    spawn_lease_renewal(&cfg, running.clone());

    // K2 Connect Pro multi-subdomain (PRD §7): when E2E is on, the daemon
    // also learns the account's nested `<label>.<sub>.k2.dev → internal
    // endpoint` map from the control plane on the same cadence, so the TLS
    // listener can route inbound by Host. No-op when E2E is off (default) or
    // the config can't drive a fetch — see `spawn_subdomain_refresh`.
    spawn_subdomain_refresh(&cfg, running.clone());

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

/// Write the rendered TOML to `~/.k2/frpc.toml` (0600) via tmp+rename.
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

/// Render frpc.toml dialing `relay` and write it into place. The relays
/// are peers (one shared frps token, one wildcard cert), so only
/// `serverAddr`/`serverPort` differ between renders — token, subdomain,
/// and proxy name are untouched by construction
/// (see [`render_frpc_toml_for_relay`]).
fn write_relay_config(
    cfg: &TunnelConfig,
    relay: &RelayEndpoint,
    local_port: u16,
    e2e: bool,
) -> Result<(), String> {
    write_config_file(&render_frpc_toml_for_relay(cfg, relay, local_port, e2e))
}

/// How one supervised child run ended (multi-relay failover accounting).
enum ChildOutcome {
    /// The child exited on its own (or `wait` errored); exit code for the
    /// restart log line.
    Exited(Option<i32>),
    /// WE killed the child deliberately to fail back to the preferred
    /// relay after a stable run on a fallback — respawn immediately, no
    /// failure counted, no backoff.
    FailBack(RelaySwitch),
}

/// Wait for the current child to end.
///
/// * Single relay ([`RelaySelector::is_solo`]) — plain blocking
///   `wait()`, byte-identical to the pre-failover supervisor: no polling,
///   no fail-back to evaluate, no behavior change.
/// * Multi-relay — poll `try_wait` on [`SUPERVISE_POLL`] so we can (a)
///   feed healthy uptime into the selector while the child lives (the
///   fail-back streak) and (b) observe `stop()` and kill the child we're
///   holding (it was TAKEN from the slot, so `stop()` can't reach it).
fn wait_for_exit(
    child: &mut Child,
    running: &AtomicBool,
    selector: &mut RelaySelector,
    spawned_at: Instant,
) -> ChildOutcome {
    if selector.is_solo() {
        return ChildOutcome::Exited(child.wait().ok().and_then(|s| s.code()));
    }
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return ChildOutcome::Exited(status.code()),
            Ok(None) => {} // still running
            Err(e) => {
                crate::log_debug!("[tunnel] wait on frpc child failed: {e}");
                return ChildOutcome::Exited(None);
            }
        }
        if !running.load(Ordering::SeqCst) {
            // Stop requested while the child lives: we hold the handle,
            // so the kill is ours to do. The caller returns right after.
            let _ = child.kill();
            let _ = child.wait();
            return ChildOutcome::Exited(None);
        }
        // A child alive past HEALTHY_UPTIME is a working relay: reset the
        // failure counter and extend the fail-back streak. The selector
        // says when the streak has earned a return to the primary.
        if spawned_at.elapsed() >= HEALTHY_UPTIME {
            if let Some(sw) = selector.on_success(Instant::now()) {
                let _ = child.kill();
                let _ = child.wait();
                return ChildOutcome::FailBack(sw);
            }
        }
        std::thread::sleep(SUPERVISE_POLL);
    }
}

/// Spawn the frpc child once and start the supervisor thread that
/// captures output and restarts on unexpected exit with backoff.
///
/// **Multi-relay failover** (`cfg.relay_list().len() > 1`): the supervisor
/// classifies each child death by uptime — under [`HEALTHY_UPTIME`] counts
/// as a relay failure, over it as a success — and feeds a [`RelaySelector`].
/// When the selector rotates (3 consecutive failures → next relay,
/// wrapping; or a stable run on a fallback → back to the primary), the
/// supervisor re-renders `frpc.toml` for the new relay before respawning
/// and resets the backoff. With a single relay the selector never switches
/// and the wait path is the original blocking `wait()` — behavior is
/// byte-identical to the pre-failover supervisor.
fn spawn_supervised(
    frpc: PathBuf,
    child_slot: Arc<Mutex<Option<Child>>>,
    running: Arc<AtomicBool>,
    cfg: &TunnelConfig,
    resolved_local_port: u16,
    e2e: bool,
) -> Result<(), String> {
    // First spawn happens synchronously so `start()` fails loud if the
    // very first launch can't even exec.
    let first = spawn_once(&frpc)?;
    *child_slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(first);

    let frpc_thread = frpc.clone();
    let cfg_thread = cfg.clone();
    std::thread::Builder::new()
        .name("k2so-frpc-supervisor".to_string())
        .spawn(move || {
            let initial_backoff = Duration::from_millis(500);
            let max_backoff = Duration::from_secs(30);
            let mut backoff = initial_backoff;
            let mut selector = RelaySelector::new(cfg_thread.relay_list());
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

                let spawned_at = Instant::now();
                let outcome = wait_for_exit(&mut child, &running, &mut selector, spawned_at);
                if !running.load(Ordering::SeqCst) {
                    // Stop requested — do not restart.
                    return;
                }
                match outcome {
                    ChildOutcome::FailBack(sw) => {
                        // Deliberate switch back to the preferred relay:
                        // re-render, reset backoff, respawn immediately
                        // (no failure happened, so no penalty sleep).
                        crate::log_debug!(
                            "[tunnel] relay fail-back: {} -> {} (stable on fallback; retrying primary)",
                            sw.from,
                            sw.to
                        );
                        if let Err(e) = write_relay_config(
                            &cfg_thread,
                            selector.current(),
                            resolved_local_port,
                            e2e,
                        ) {
                            crate::log_debug!(
                                "[tunnel] WARN: rewrite frpc.toml for {} failed: {e}",
                                selector.current()
                            );
                        }
                        backoff = initial_backoff;
                    }
                    ChildOutcome::Exited(code) => {
                        // Classify the run for the selector: a quick death
                        // is a failed dial on the current relay, a long run
                        // was a working connection (resets the counter, and
                        // on a fallback may complete the fail-back streak).
                        let switch = if spawned_at.elapsed() >= HEALTHY_UPTIME {
                            selector.on_success(Instant::now())
                        } else {
                            selector.on_failure()
                        };
                        if let Some(sw) = switch {
                            if sw.is_failback() {
                                crate::log_debug!(
                                    "[tunnel] relay fail-back: {} -> {} (stable on fallback; retrying primary)",
                                    sw.from,
                                    sw.to
                                );
                            } else {
                                crate::log_debug!(
                                    "[tunnel] relay failover: {} -> {} after {} failures",
                                    sw.from,
                                    sw.to,
                                    sw.after_failures
                                );
                            }
                            if let Err(e) = write_relay_config(
                                &cfg_thread,
                                selector.current(),
                                resolved_local_port,
                                e2e,
                            ) {
                                crate::log_debug!(
                                    "[tunnel] WARN: rewrite frpc.toml for {} failed: {e}",
                                    selector.current()
                                );
                            }
                            // Fresh target — dial it promptly instead of
                            // inheriting the dead relay's grown backoff.
                            backoff = initial_backoff;
                        }
                        crate::log_debug!(
                            "[tunnel] frpc exited ({:?}); restarting in {:?}",
                            code,
                            backoff
                        );
                        std::thread::sleep(backoff);
                        backoff = (backoff * 2).min(max_backoff);
                    }
                }
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
///
/// K2 Cloud P1-C — availability is decided ONCE here, per tunnel start,
/// instead of failing (and logging) on every 60 s tick:
///   * `K2_TUNNEL_LEASE=off` is honored before any keychain probe;
///   * no account session material (headless/hosted/provisioned daemons,
///     non-macOS, or a Mac that never signed in) → a LOUD-ONCE skip. The
///     tunnel itself is unaffected — the lease only powers the "which
///     device holds this subdomain" holder UI, and K2 Cloud rows are
///     single-holder by construction.
/// The disabled log fires at most once per PROCESS (not per tunnel
/// restart); a later sign-in is picked up on the next tunnel start because
/// the mode is re-evaluated each spawn.
fn spawn_lease_renewal(cfg: &TunnelConfig, running: Arc<AtomicBool>) {
    // Target check FIRST: it's pure config (touches no keychain), and a
    // token-only manual config skips before we'd probe anything.
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

    // Availability: env kill-switch (checked inside BEFORE any keychain
    // probe), then session presence.
    static DISABLED_LOGGED: std::sync::Once = std::sync::Once::new();
    match super::lease::renewal_mode() {
        super::lease::RenewalMode::Enabled => {}
        super::lease::RenewalMode::DisabledByEnv => {
            DISABLED_LOGGED.call_once(|| {
                crate::log_debug!(
                    "[tunnel/lease] subdomain lease renewal disabled via {}=off",
                    super::lease::LEASE_ENV_VAR
                );
            });
            return;
        }
        super::lease::RenewalMode::NoSession => {
            DISABLED_LOGGED.call_once(|| {
                crate::log_debug!(
                    "[tunnel/lease] subdomain lease renewal disabled — no account session \
                     (normal for provisioned/hosted daemons)"
                );
            });
            return;
        }
    }

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

/// K2 Connect Pro multi-subdomain (PRD §7) — spawn the daemon-owned loop
/// that learns the account's nested `<label>.<sub>.k2.dev → internal
/// endpoint` map from the control plane and caches it for the E2E TLS
/// listener's Host routing.
///
/// **Gated on E2E** ([`super::config::e2e_enabled`]): the nested-subdomain
/// routing only exists on the daemon-terminated-TLS path, so when E2E is OFF
/// (the default) this is a no-op and nothing is fetched — byte-for-byte the
/// existing behaviour. Also a no-op when the config carries no subdomain
/// label or no bearer token (nothing to fetch / no auth).
///
/// Lifecycle mirrors the lease loop: watches the same `running` flag and
/// self-exits on [`stop`]. A fetch failure is logged + retried next tick; the
/// previously-cached map keeps serving meanwhile (never drops routing on a
/// transient blip). Refreshes on [`super::lease::RENEW_INTERVAL`] — the same
/// cadence the lease loop uses.
fn spawn_subdomain_refresh(cfg: &TunnelConfig, running: Arc<AtomicBool>) {
    if !super::config::e2e_enabled(cfg) {
        return; // OFF (default) → no nested routing, no fetch. Unchanged.
    }
    let primary = cfg.subdomain.trim().to_string();
    let token = cfg.token.trim().to_string();
    if primary.is_empty() || token.is_empty() {
        crate::log_debug!(
            "[tunnel/subdomains] E2E on but no subdomain/token in config — \
             skipping daemon-side subdomain-map refresh"
        );
        return;
    }

    let spawned = std::thread::Builder::new()
        .name("k2so-tunnel-subdomains".to_string())
        .spawn(move || {
            crate::log_debug!(
                "[tunnel/subdomains] daemon-owned subdomain-map refresh started for {} (every {:?})",
                primary,
                super::lease::RENEW_INTERVAL
            );
            loop {
                if !running.load(Ordering::SeqCst) {
                    break;
                }
                match super::subdomains::refresh_once(&primary, &token) {
                    Ok((n, _changed)) => crate::log_debug!(
                        "[tunnel/subdomains] refreshed {n} nested target(s) for {primary}"
                    ),
                    Err(e) => crate::log_debug!(
                        "[tunnel/subdomains] refresh tick failed (keeping cached map, \
                         will retry next interval): {e}"
                    ),
                }
                // Sleep in short slices so `stop()` is observed within ~1s.
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
            crate::log_debug!("[tunnel/subdomains] subdomain-map refresh stopped for {primary}");
        });
    if let Err(e) = spawned {
        crate::log_debug!(
            "[tunnel/subdomains] WARN: failed to spawn subdomain-map refresh thread: {e}"
        );
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

    /// A real, always-present no-op executable to stand in for `frpc` when a
    /// test needs to get PAST frpc resolution (and possibly spawn a harmless
    /// child). `/usr/bin/true` on macOS; `/bin/true` on Linux.
    fn true_bin() -> PathBuf {
        for cand in ["/usr/bin/true", "/bin/true"] {
            let p = PathBuf::from(cand);
            if p.exists() {
                return p;
            }
        }
        panic!("no `true` binary found at /usr/bin/true or /bin/true");
    }

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
            // Opt out of E2E (e2e:false) so this test exercises the frpc
            // resolution path directly — with E2E on (the default) the start
            // would first try to ensure the HTTPS listener (no daemon hook in
            // this unit context) and surface that error instead.
            config::save(&TunnelConfig {
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                e2e: false,
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

    /// E2E default-on path: with no daemon ensure-hook registered (a pure
    /// k2-core unit context) and no published HTTPS port, a start on an
    /// E2E config must FAIL LOUD rather than forwarding cleartext to the
    /// HTTP port. Proves the connector consults `ensure_https_port` and
    /// propagates its error.
    #[test]
    fn e2e_start_without_https_listener_fails_loud_not_cleartext() {
        with_temp_home(|| {
            // Default config → e2e is ON. A real frpc path (/bin/true) so we
            // get PAST frpc resolution and reach the HTTPS-port resolution.
            let prev = std::env::var_os("K2_E2E");
            std::env::remove_var("K2_E2E"); // follow config (default-on)
            config::save(&TunnelConfig {
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                ..Default::default() // e2e defaults true
            })
            .expect("seed config");

            let err = start(None, 57839, &FrpcBinary::Explicit(true_bin()))
                .expect_err("E2E start with no HTTPS listener must fail loud");
            assert!(
                err.contains("HTTPS listener") || err.contains("leak plaintext"),
                "expected an HTTPS-listener/anti-cleartext error, got: {err}"
            );
            assert!(!status().running, "no child should be running after the loud failure");

            match prev {
                Some(p) => std::env::set_var("K2_E2E", p),
                None => std::env::remove_var("K2_E2E"),
            }
        });
    }

    /// E2E default-on + a published HTTPS port (simulating the daemon's
    /// listener being up): the connector resolves frpc's localPort to the
    /// HTTPS port, not the cleartext HTTP `default_local_port`. We force frpc
    /// resolution to fail right AFTER port resolution so we don't actually
    /// spawn, and assert the rendered config targeted the HTTPS port.
    #[test]
    fn e2e_start_resolves_https_port_when_published() {
        with_temp_home(|| {
            let prev = std::env::var_os("K2_E2E");
            std::env::remove_var("K2_E2E");
            config::save(&TunnelConfig {
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                ..Default::default()
            })
            .expect("seed config");
            // Simulate the daemon having published its HTTPS listener port.
            super::super::tls::publish_https_port(48217).expect("publish https port");

            // frpc missing → start fails AFTER resolving the HTTPS port, but
            // it must have written frpc.toml targeting the HTTPS port first?
            // No: render happens after frpc resolves. So instead use a real
            // binary so we reach render, then inspect the written config.
            let _ = start(None, 57839, &FrpcBinary::Explicit(true_bin()));
            let toml = std::fs::read_to_string(frpc_config_path())
                .expect("frpc.toml must have been rendered");
            assert!(
                toml.contains("localPort = 48217"),
                "E2E start must target the published HTTPS port, not the HTTP port\n{toml}"
            );
            assert!(
                toml.contains("type = \"https\""),
                "E2E start must render an https proxy\n{toml}"
            );

            let _ = stop();
            match prev {
                Some(p) => std::env::set_var("K2_E2E", p),
                None => std::env::remove_var("K2_E2E"),
            }
        });
    }

    /// Multi-relay failover, end to end through the REAL supervise loop:
    /// a two-relay config whose "frpc" (/bin/true) exits instantly — i.e.
    /// every dial to the primary "fails" fast — must, after 3 consecutive
    /// failures, rewrite frpc.toml to dial the SECOND relay. Everything
    /// else in the rendered config (token, subdomain, proxy name) must be
    /// untouched by the rotation.
    #[test]
    fn supervisor_rotates_to_second_relay_after_three_fast_failures() {
        with_temp_home(|| {
            config::save(&TunnelConfig {
                relays: vec![
                    RelayEndpoint { host: "10.0.0.1".to_string(), port: 7000 },
                    RelayEndpoint { host: "10.0.0.2".to_string(), port: 7000 },
                ],
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                e2e: false, // skip the HTTPS-listener path in this unit context
                ..Default::default()
            })
            .expect("seed two-relay config");

            let st = start(None, 57839, &FrpcBinary::Explicit(true_bin()))
                .expect("start with two relays");
            assert!(st.running);

            // The first render must dial the PRIMARY relay.
            let toml = std::fs::read_to_string(frpc_config_path()).expect("frpc.toml");
            assert!(
                toml.contains("serverAddr = \"10.0.0.1\""),
                "must dial the primary first\n{toml}"
            );

            // /bin/true exits immediately → each supervised run is a fast
            // failure. With SUPERVISE_POLL=1s and backoffs 0.5s/1s, the 3rd
            // failure (and the rewrite to relay #2) lands within a few
            // seconds; poll generously so slow CI can't flake this.
            let deadline = Instant::now() + Duration::from_secs(30);
            let rotated = loop {
                let toml = std::fs::read_to_string(frpc_config_path()).expect("frpc.toml");
                if toml.contains("serverAddr = \"10.0.0.2\"") {
                    break toml;
                }
                if Instant::now() >= deadline {
                    panic!(
                        "supervisor never rotated to the second relay; frpc.toml:\n{toml}"
                    );
                }
                std::thread::sleep(Duration::from_millis(200));
            };

            // Only the server endpoint rotates — token/subdomain/proxy name
            // are byte-identical (the relays are peers).
            assert!(rotated.contains("token = \"tok\""), "{rotated}");
            assert!(rotated.contains("subdomain = \"rosson\""), "{rotated}");
            assert!(rotated.contains("name = \"k2so-rosson\""), "{rotated}");

            stop().expect("stop");
            assert!(!status().running);
        });
    }

    /// Single-relay config through the SAME start path: the rendered TOML
    /// must dial the legacy `server_addr`/`server_port` pair (relay_list()
    /// folds it in), and no rotation can ever occur — there is nowhere to
    /// rotate to. Guards the "byte-identical when one relay" contract at
    /// the connector level.
    #[test]
    fn single_relay_start_renders_legacy_endpoint_unchanged() {
        with_temp_home(|| {
            config::save(&TunnelConfig {
                server_addr: "9.9.9.9".to_string(),
                server_port: 7009,
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                e2e: false,
                ..Default::default()
            })
            .expect("seed legacy config");

            let _ = start(None, 57839, &FrpcBinary::Explicit(true_bin()));
            let toml = std::fs::read_to_string(frpc_config_path()).expect("frpc.toml");
            assert!(
                toml.contains("serverAddr = \"9.9.9.9\""),
                "legacy endpoint must render unchanged\n{toml}"
            );
            assert!(toml.contains("serverPort = 7009"), "{toml}");
            let _ = stop();
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
        println!(
            "{}",
            super::super::render::render_frpc_toml(&cfg, 57839, false)
        );
    }

    /// REAL-PROCESS, REAL-NETWORK test — gated `#[ignore]` so `cargo
    /// test` never spawns a live frpc against the production frps box
    /// (which would collide with the parent's live validation). Run
    /// manually only, with a real token in ~/.k2/tunnel.json and frpc
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
