//! TunnelConnector — launches and supervises the `frpc` child process
//! that dials the K2 Connect frps server.
//!
//! Lifecycle:
//!   * `start()` resolves the `frpc` binary, renders the config TOML to a
//!     0600 file under `~/.k2/`, reaps any stray frpc bound to that config,
//!     spawns `frpc -c <file>`, and starts a supervisor thread that
//!     captures stdout/stderr to a log and restarts the child on unexpected
//!     exit with exponential backoff.
//!   * `stop()` flips the desired-state flag, kills any Child still in the
//!     slot, always reaps stray frpc by config pattern (the supervisor
//!     `.take()`s the handle while waiting — slot kill alone orphans), and
//!     the supervisor poll observes the flag and does NOT restart.
//!   * `status()` reports running/stopped + the predicted public URL.
//!
//! The connector is a process-wide singleton (one tunnel per daemon),
//! held behind a `Mutex` in [`STATE`]. The binary path is pluggable
//! ([`FrpcBinary`]) so tests can inject a fake and production can locate
//! `frpc` via PATH or common install dirs.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::config::{self, RelayEndpoint, TunnelConfig, SUBDOMAIN_HOST};
use super::failover::{RelaySelector, RelaySwitch};
use super::render::render_frpc_toml_for_relay;
use super::watchdog::DisconnectTracker;

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

/// Filenames for the frpc client on this OS (Windows ships `frpc.exe`).
fn frpc_basenames() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &["frpc.exe", "frpc"]
    }
    #[cfg(not(windows))]
    {
        &["frpc"]
    }
}

/// Common non-PATH locations to probe for a `frpc` install.
///
/// Includes the desktop app install dir (sibling of `k2` / `k2-daemon`) so
/// NSIS/macOS bundles work without a separate package-manager install, and
/// `~/.k2/bin` where the thin client stages the bundled sidecar on launch.
fn common_frpc_locations() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    // Absolute system probes are production-only so unit tests that empty
    // PATH + temp HOME stay deterministic on developer machines with brew.
    #[cfg(all(not(windows), not(test)))]
    {
        v.push(PathBuf::from("/opt/homebrew/bin/frpc"));
        v.push(PathBuf::from("/usr/local/bin/frpc"));
        v.push(PathBuf::from("/usr/bin/frpc"));
    }
    // Bundled install: next to the running k2 / k2-daemon binary
    // (`%LOCALAPPDATA%\K2\frpc.exe` on Windows, Contents/MacOS/frpc on macOS).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in frpc_basenames() {
                v.push(dir.join(name));
            }
        }
    }
    if let Some(home) = dirs::home_dir() {
        for name in frpc_basenames() {
            v.push(home.join(".local/bin").join(name));
            v.push(home.join(".k2/bin").join(name));
            // Legacy location pre-`.k2so`→`.k2` cutover.
            v.push(home.join(".k2so/bin").join(name));
        }
    }
    v
}

/// Resolve the `frpc` executable, or a clear "not installed" error.
/// Does NOT auto-download — the desktop app stages a bundled sidecar; this
/// only *finds* it (or a user-installed copy).
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
            // 2) Common install dirs + next to current exe (bundled).
            for cand in common_frpc_locations() {
                if cand.exists() {
                    return Ok(cand);
                }
            }
            Err(
                "frpc not installed: the K2 Connect tunnel requires the `frpc` \
                 client binary (fatedier/frp v0.61+). Desktop installs of K2 ship \
                 it next to the app and stage it under ~/.k2/bin — reinstall or \
                 update K2 if this is missing. Otherwise install via your package \
                 manager (e.g. `brew install frpc`) or download a release from \
                 https://github.com/fatedier/frp/releases and place it on your PATH."
                    .to_string(),
            )
        }
    }
}

/// Minimal PATH lookup (no external `which` dep). Returns the first
/// executable `name` found in the `$PATH` directories.
///
/// On Windows also tries `name.exe` (CreateProcess PATHEXT is not applied
/// to raw path existence checks).
fn which_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for base in frpc_path_candidates(name) {
            let cand = dir.join(base);
            if is_executable(&cand) {
                return Some(cand);
            }
        }
    }
    None
}

fn frpc_path_candidates(name: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        let mut out = vec![name.to_string()];
        if !name.ends_with(".exe") && !name.ends_with(".EXE") {
            out.push(format!("{name}.exe"));
        }
        out
    }
    #[cfg(not(windows))]
    {
        vec![name.to_string()]
    }
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

/// Path the rendered frpc config is written to. `pub(crate)` so a
/// Release ([`super::unpair`]) can delete it — the rendered TOML embeds
/// the bearer token, so it is identity material too.
pub(crate) fn frpc_config_path() -> PathBuf {
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

/// Poll cadence while waiting on the child (solo stop-observability and
/// multi-relay failover/watchdog). Matches the 1 s slice the lease /
/// subdomain loops use to observe `stop()` promptly.
const SUPERVISE_POLL: Duration = Duration::from_secs(1);

/// How often the supervisor probes whether frpc's local target is still
/// reachable on loopback (Bug B self-heal / port-desync). 10× SUPERVISE_POLL.
const LOCAL_TARGET_PROBE_EVERY: u32 = 10;

/// TCP connect budget when probing `127.0.0.1:<localPort>` for self-heal.
const LOCAL_TARGET_PROBE_TIMEOUT: Duration = Duration::from_millis(400);

/// Live connector state — the supervised child + the desired-state flag.
struct ConnectorState {
    /// The currently-running config.
    cfg: TunnelConfig,
    /// frpc `localPort` — frozen at start (R1) and only rewritten by the
    /// self-heal path when the frozen target is unreachable. Shared so
    /// `status()` and the supervisor always agree after a heal.
    resolved_local_port: Arc<AtomicU16>,
    /// The frpc child handle. `None` between restarts.
    child: Arc<Mutex<Option<Child>>>,
    /// Desired state: `true` = should be running (supervisor restarts on
    /// exit); `false` = stop requested (supervisor must not restart).
    running: Arc<AtomicBool>,
    /// The relay the supervisor is CURRENTLY homed to — index 0 of the
    /// fallback list at start, republished by the supervise loop on every
    /// failover/fail-back so `status()` reports the LIVE relay rather than
    /// the configured primary. A single-relay config never rotates, so
    /// this stays the legacy endpoint and status is unchanged.
    current_relay: Arc<Mutex<RelayEndpoint>>,
}

static STATE: OnceLock<Mutex<Option<ConnectorState>>> = OnceLock::new();

fn state() -> &'static Mutex<Option<ConnectorState>> {
    STATE.get_or_init(|| Mutex::new(None))
}

/// Why the ON-DISK spawn gate refused to arm frpc (PRD
/// `prd-tunnel-disable-unpair-v1.md`). Both variants are TERMINAL for the
/// supervisor: it must exit, not backoff-retry — a refused zombie hammering
/// the relay every 33s forever is the pre-mortem this kills.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SpawnBlock {
    /// `tunnel.json` `enabled: false` — the user's persisted PAUSE.
    Disabled,
    /// The identity on disk matches the `unpaired.json` tombstone (or the
    /// tunnel state is unreadable, which fails CLOSED) — this device
    /// released its subdomain and can never re-arm with that identity.
    Released,
    /// `tunnel.json` exists but can't be read/parsed. Fail closed: a
    /// corrupt secret store must never spawn a tunnel.
    Unreadable(String),
    /// Air-gap flag is on — no frpc, no Connect.
    Airgap,
}

impl std::fmt::Display for SpawnBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnBlock::Disabled => write!(
                f,
                "tunnel is disabled on this device (persisted in tunnel.json) — \
                 re-enable with `k2 tunnel enable` or the Settings → K2 Connect toggle"
            ),
            SpawnBlock::Released => write!(
                f,
                "this device's tunnel identity was released (unpaired) — it can no \
                 longer claim its old subdomain; re-pair in Settings → K2 Connect to \
                 mint a fresh identity"
            ),
            SpawnBlock::Unreadable(e) => write!(f, "tunnel state unreadable: {e}"),
            SpawnBlock::Airgap => write!(f, "{}", crate::airgap::TEACHING),
        }
    }
}

/// The ON-DISK gate every frpc spawn must pass (PRD §2 pre-mortem: "the
/// flag must be read at frpc-SPAWN time by whatever process spawns frpc —
/// no cached copy. Kill the class, not the instance"). Reads `tunnel.json`
/// + the `unpaired.json` tombstone FRESH from disk on every call, so a
/// restarted daemon, a rebooted machine, and an orphaned second daemon all
/// observe a disable/release they never saw happen.
///
/// Checked in [`start`] (boot autostart + every route/CLI start) and by
/// the supervisor before EVERY respawn — a disable or release landing
/// mid-flight stops the tunnel at the next child exit and is never undone
/// by a stale in-memory copy.
fn spawn_gate() -> Result<(), SpawnBlock> {
    if crate::airgap::enabled() {
        return Err(SpawnBlock::Airgap);
    }
    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => return Err(SpawnBlock::Unreadable(e)),
    };
    // Released beats disabled: a tombstoned identity is permanent.
    if super::unpair::identity_released(&cfg).is_some() {
        return Err(SpawnBlock::Released);
    }
    if !cfg.enabled {
        return Err(SpawnBlock::Disabled);
    }
    Ok(())
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
    /// The persisted PAUSE flag (PRD tunnel-disable-unpair §2A), read from
    /// disk: `false` = the user disabled the tunnel on this device and no
    /// frpc will spawn until re-enabled. Always emitted so the UI can show
    /// "Disabled (by you)" instead of a generic "stopped".
    pub enabled: bool,
    /// True when this device is in the RELEASED (unpaired) state — the
    /// identity was deleted and tombstoned (PRD §2B). Always emitted so
    /// the UI's tri-state (Connected / Disabled / Released) never guesses.
    pub released: bool,
}

impl TunnelStatus {
    fn stopped() -> Self {
        // Disk truth for the tri-state: the connector singleton being
        // empty says nothing about WHY the tunnel is down.
        let cfg = config::load().unwrap_or_default();
        Self {
            running: false,
            public_url: None,
            subdomain: None,
            local_port: None,
            server_addr: None,
            frpc_installed: resolve_frpc(&FrpcBinary::Auto).is_ok(),
            enabled: cfg.enabled,
            released: super::unpair::released_state(&cfg),
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

    // The ON-DISK gate FIRST (PRD tunnel-disable-unpair): a disabled or
    // released device never spawns frpc, regardless of relay list or how
    // the start was reached (boot autostart, route, CLI). Checked before
    // config reconciliation so a disabled tunnel reports "disabled", not
    // a misleading "not configured".
    if let Err(block) = spawn_gate() {
        return Err(block.to_string());
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
    // Live E2E listener port is the single source of truth for frpc
    // localPort (Bug B / #55). NEVER invent a free port or re-read a
    // stale file over a live OnceLock — `ensure_https_port` prefers the
    // daemon-registered hook (process-local bound port).
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

    // Invariant: after render, frpc.toml localPort must match the live
    // resolution we just froze (debug-visible in journal on mismatch).
    if let Err(msg) = tunnel_port_invariant_ok(
        e2e,
        if e2e { Some(resolved_local_port) } else { None },
        super::tls::read_https_port(),
        parse_local_port_from_frpc_toml(
            &std::fs::read_to_string(frpc_config_path()).unwrap_or_default(),
        ),
        Some(resolved_local_port),
    ) {
        crate::log_debug!("[tunnel] WARN: port invariant after start render: {msg}");
    }

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
    let resolved_port_slot = Arc::new(AtomicU16::new(resolved_local_port));
    // The live-relay slot starts at the preferred relay (index 0 — what
    // was just rendered above); the supervise loop republishes it on every
    // rotation so status() always names the relay actually being dialed.
    let current_relay = Arc::new(Mutex::new(cfg.relay_list()[0].clone()));
    spawn_supervised(
        frpc,
        child.clone(),
        running.clone(),
        current_relay.clone(),
        &cfg,
        resolved_port_slot.clone(),
        default_local_port,
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
        resolved_local_port: resolved_port_slot,
        child,
        running,
        current_relay,
    };
    let status = status_from(&st);
    *guard = Some(st);
    Ok(status)
}

/// Stop the tunnel. Flips desired-state to stopped (so the supervisor
/// won't restart), kills any live child handle still in the slot, and
/// always reaps stray frpc by config path — the supervisor `.take()`s
/// the Child while waiting, so a bare `st.child` kill is often a no-op
/// and orphans frpc without the pattern reap. Idempotent — stopping a
/// stopped tunnel is `Ok`.
///
/// Hardened (Bug B / Phase 4b): `running=false` FIRST, slot kill, pattern
/// reap, short re-check, second reap (and SIGKILL if still alive) so a
/// graceful daemon exit under `KillMode=process` cannot leave orphan frpc.
pub fn stop() -> Result<(), String> {
    let mut guard = state().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(st) = guard.as_ref() {
        // 1) Tell the supervisor not to respawn — MUST be first.
        st.running.store(false, Ordering::SeqCst);
        // 2) Kill the Child handle if the supervisor hasn't taken it yet.
        if let Some(child) = st.child.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
            // Best-effort kill. frpc has no special signal protocol.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    // 3) Always reap by config pattern — independent of Child ownership.
    //    Covers: supervisor-held child (slot empty), orphans from prior
    //    crashes, and solo-mode waits that never observed `running=false`
    //    before this fix. Safe when nothing matches (pkill no-match = ok).
    let cfg_path = frpc_config_path();
    crate::log_debug!(
        "[tunnel] stop: reaping stray frpc matching `{}`",
        stray_frpc_pattern(&cfg_path)
    );
    reap_stray_frpc(&cfg_path);
    // 4) Short settle + second reap — SIGTERM from pkill may not have
    //    reaped yet; a second pass catches late-dying children.
    std::thread::sleep(Duration::from_millis(150));
    reap_stray_frpc(&cfg_path);
    // 5) Last resort on unix: if a match is still alive, SIGKILL.
    #[cfg(unix)]
    {
        if stray_frpc_alive(&cfg_path) {
            crate::log_debug!(
                "[tunnel] stop: stray frpc still alive after SIGTERM — SIGKILL `{}`",
                stray_frpc_pattern(&cfg_path)
            );
            let _ = Command::new("pkill")
                .arg("-9")
                .arg("-f")
                .arg(stray_frpc_pattern(&cfg_path))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    // 6) Clear connector state so status() reports stopped.
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
    // Report the LIVE relay (the supervise loop republishes the slot on
    // every failover/fail-back), not the configured primary. For a legacy
    // single-endpoint config the slot IS server_addr — never rotated — so
    // the reported value is byte-identical to the pre-failover status.
    let server_addr = st
        .current_relay
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .host
        .clone();
    TunnelStatus {
        running: st.running.load(Ordering::SeqCst),
        public_url,
        subdomain: (!sub.is_empty()).then(|| sub.to_string()),
        local_port: Some(st.resolved_local_port.load(Ordering::SeqCst)),
        server_addr: Some(server_addr),
        frpc_installed: resolve_frpc(&FrpcBinary::Auto).is_ok(),
        // A live connector implies the gate passed at spawn time.
        enabled: true,
        released: false,
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

/// True when a process matching our frpc config pattern is still alive
/// (`pgrep -f` exit 0). Used by the hardened stop path for a second-pass
/// SIGKILL. Missing `pgrep` → false (don't escalate blindly).
#[cfg(unix)]
fn stray_frpc_alive(cfg_path: &Path) -> bool {
    Command::new("pgrep")
        .arg("-f")
        .arg(stray_frpc_pattern(cfg_path))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Parse `localPort = N` from a rendered frpc.toml body. Pure — used by
/// the port invariant helper and tests.
pub fn parse_local_port_from_frpc_toml(toml: &str) -> Option<u16> {
    for line in toml.lines() {
        let line = line.trim();
        // Accept `localPort = 12345` (renderer's exact shape).
        let rest = match line.strip_prefix("localPort") {
            Some(r) => r.trim_start(),
            None => continue,
        };
        let rest = match rest.strip_prefix('=') {
            Some(r) => r.trim(),
            None => continue,
        };
        // Drop trailing comments if any.
        let num = rest.split_whitespace().next().unwrap_or(rest);
        if let Ok(p) = num.parse::<u16>() {
            if p != 0 {
                return Some(p);
            }
        }
    }
    None
}

/// TCP-probe whether anything is accepting on `127.0.0.1:port`. Used by
/// the supervisor self-heal path to detect "frpc dials a dead localPort".
pub fn local_port_reachable(port: u16) -> bool {
    if port == 0 {
        return false;
    }
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&addr, LOCAL_TARGET_PROBE_TIMEOUT).is_ok()
}

/// Ok if live/published E2E port agrees with frpc.toml `localPort` (and the
/// connector's frozen port) when E2E is on. Pure check for tests + post-start
/// diagnostics — never invents ports.
///
/// * `e2e` — effective E2E flag for this tunnel.
/// * `live_e2e_port` — process-local bound listener (OnceLock), when known.
/// * `published_port` — contents of `~/.k2/tunnel-https.port`, if any.
/// * `frpc_local_port` — `localPort` from rendered frpc.toml, if any.
/// * `frozen_port` — connector's frozen localPort (status/supervisor).
///
/// When E2E is off, only `frpc_local_port` and `frozen_port` are compared
/// (if both present); live/published HTTPS ports are ignored.
pub fn tunnel_port_invariant_ok(
    e2e: bool,
    live_e2e_port: Option<u16>,
    published_port: Option<u16>,
    frpc_local_port: Option<u16>,
    frozen_port: Option<u16>,
) -> Result<(), String> {
    if !e2e {
        if let (Some(frpc), Some(frozen)) = (frpc_local_port, frozen_port) {
            if frpc != frozen {
                return Err(format!(
                    "frpc localPort {frpc} != frozen connector port {frozen} (E2E off)"
                ));
            }
        }
        return Ok(());
    }

    // Prefer the process-local live listener; fall back to the published file.
    let sot = match (live_e2e_port, published_port) {
        (Some(live), Some(pub_p)) if live != pub_p => {
            return Err(format!(
                "live E2E port {live} != published tunnel-https.port {pub_p}"
            ));
        }
        (Some(live), _) => live,
        (None, Some(pub_p)) => pub_p,
        (None, None) => {
            // No SoT available — still check frpc vs frozen if both present.
            if let (Some(frpc), Some(frozen)) = (frpc_local_port, frozen_port) {
                if frpc != frozen {
                    return Err(format!(
                        "frpc localPort {frpc} != frozen port {frozen} (no live/published E2E port)"
                    ));
                }
            }
            return Ok(());
        }
    };

    if let Some(frpc) = frpc_local_port {
        if frpc != sot {
            return Err(format!(
                "frpc localPort {frpc} != live/published E2E port {sot}"
            ));
        }
    }
    if let Some(frozen) = frozen_port {
        if frozen != sot {
            return Err(format!(
                "frozen connector port {frozen} != live/published E2E port {sot}"
            ));
        }
    }
    Ok(())
}

/// Re-resolve the port frpc should dial from the **live** E2E listener
/// (preferred) without inventing a free port. Used by self-heal only —
/// normal respawns keep the frozen value (R1).
fn re_resolve_live_local_port(e2e: bool, daemon_http_port: u16, current: u16) -> u16 {
    if !e2e {
        return current;
    }
    // ensure_https_port → daemon hook (liveness-aware re-bind if accept loop
    // died) → live port. Fall back to the published file, then the freeze.
    match super::tls::ensure_https_port(daemon_http_port) {
        Ok(p) if p != 0 => {
            // Re-publish so tunnel-https.port can't drift from the live port.
            let _ = super::tls::publish_https_port(p);
            p
        }
        Ok(_) | Err(_) => {
            if let Some(p) = super::tls::read_https_port() {
                p
            } else {
                current
            }
        }
    }
}

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
    /// The mid-session watchdog declared the relay dead (frpc alive but
    /// storming reconnect errors — it never exits on a post-login relay
    /// death) and WE killed the child. Always counts as a relay FAILURE:
    /// the generic uptime classification would wrongly credit the long
    /// pre-death run as a success and never rotate.
    WatchdogKill,
    /// Loopback probe found frpc's localPort unreachable while the tunnel
    /// should still be up (Bug B desync: live E2E on port A, frpc dials B).
    /// Supervisor re-resolves the live port, rewrites frpc.toml, and
    /// respawns frpc only — no daemon restart, agent PTYs survive.
    LocalPortUnreachable,
}

/// Shared per-child-run state for the MID-SESSION relay-death watchdog.
///
/// frpc self-exits only on a failed LOGIN (`loginFailExit` defaults
/// true) — when an ESTABLISHED session's relay dies, frpc stays alive and
/// reconnect-retries internally forever, so the supervise loop's
/// exit-based failover never fires. The pump threads feed every frpc log
/// line through a [`DisconnectTracker`]; when it declares the session
/// dead the poll loop kills the child, CONVERTING the stuck session into
/// the exit the existing failover path already handles. One instance per
/// spawned child — a fresh child starts with a clean streak.
struct SessionWatch {
    /// Pure death-detection policy, fed by BOTH pump threads (stdout +
    /// stderr) — hence the mutex; contention is two line-rate readers.
    tracker: Mutex<DisconnectTracker>,
    /// Latched by the pump side once the tracker declares death; the
    /// supervise poll loop observes it and kills the child.
    dead: AtomicBool,
}

impl SessionWatch {
    fn new() -> Self {
        Self {
            tracker: Mutex::new(DisconnectTracker::new()),
            dead: AtomicBool::new(false),
        }
    }

    /// Feed one frpc log line (called from the pump threads).
    fn observe_line(&self, line: &str) {
        if self.dead.load(Ordering::SeqCst) {
            return; // already declared — nothing more to learn
        }
        let mut tracker = self.tracker.lock().unwrap_or_else(|p| p.into_inner());
        if tracker.observe(line, Instant::now()) {
            self.dead.store(true, Ordering::SeqCst);
        }
    }

    fn is_dead(&self) -> bool {
        self.dead.load(Ordering::SeqCst)
    }

    /// (threshold, window) of the policy — for the kill log line.
    fn policy(&self) -> (usize, Duration) {
        let tracker = self.tracker.lock().unwrap_or_else(|p| p.into_inner());
        (tracker.threshold(), tracker.window())
    }
}

/// Wait for the current child to end.
///
/// * Single relay ([`RelaySelector::is_solo`]) — poll `try_wait` so we can
///   observe `stop()` and kill the child we're holding. The Child was
///   TAKEN from the slot, so `stop()` can't reach the handle; without
///   this poll a solo frpc is orphaned on SIGTERM/Stop until the next
///   `start()` reaps it. **No** mid-session watchdog and **no** fail-back
///   on solo — with a single relay there is nowhere to rotate, and a
///   reconnect-looping frpc is left to frp's own retry (stop still kills
///   via the poll + `reap_stray_frpc` on the stop path). The pre-failover
///   "byte-identical solo wait" tradeoff is intentionally broken here
///   for stop correctness: a bare `child.wait()` never saw `running=false`.
/// * Multi-relay — same poll loop plus (a) healthy-uptime feed into the
///   selector (fail-back streak), (b) stop observability, and (c) the
///   mid-session watchdog ([`SessionWatch`]) for live-but-stuck children
///   whose relay died AFTER login (frpc never exits on its own then).
fn wait_for_exit(
    child: &mut Child,
    running: &AtomicBool,
    selector: &mut RelaySelector,
    spawned_at: Instant,
    watch: Option<&SessionWatch>,
    local_port: &AtomicU16,
    e2e: bool,
    daemon_http_port: u16,
) -> ChildOutcome {
    let mut ticks_since_probe = 0u32;
    // Solo: stop-observability + localPort self-heal — no watchdog / fail-back.
    if selector.is_solo() {
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return ChildOutcome::Exited(status.code()),
                Ok(None) => {}
                Err(e) => {
                    crate::log_debug!("[tunnel] wait on frpc child failed: {e}");
                    return ChildOutcome::Exited(None);
                }
            }
            if !running.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
                return ChildOutcome::Exited(None);
            }
            if probe_local_desync(
                &mut ticks_since_probe,
                local_port,
                e2e,
                daemon_http_port,
            ) {
                let _ = child.kill();
                let _ = child.wait();
                return ChildOutcome::LocalPortUnreachable;
            }
            std::thread::sleep(SUPERVISE_POLL);
        }
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
        // Mid-session relay death: the watchdog (fed by the pump threads)
        // declared the session dead — kill the stuck child so the normal
        // exit path runs. Checked BEFORE the healthy-uptime feed below so
        // a just-declared death can't credit one more success tick.
        if let Some(w) = watch {
            if w.is_dead() {
                let (n, window) = w.policy();
                crate::log_debug!(
                    "[tunnel] mid-session relay death detected ({n} errors/{}s) — cycling frpc",
                    window.as_secs()
                );
                let _ = child.kill();
                let _ = child.wait();
                return ChildOutcome::WatchdogKill;
            }
        }
        // Bug B self-heal: only when frozen localPort is dead AND a
        // different live E2E port is available (true desync). Same-port
        // unreachable (listener down / test fixture) does not thrash.
        if probe_local_desync(
            &mut ticks_since_probe,
            local_port,
            e2e,
            daemon_http_port,
        ) {
            let _ = child.kill();
            let _ = child.wait();
            return ChildOutcome::LocalPortUnreachable;
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

/// Periodic localPort desync / E2E-listener-death probe. Returns true when
/// the frozen port is unreachable and we obtained a **reachable** live E2E
/// port (possibly after the daemon hook re-bound a dead listener — luzz
/// 2026-07-29 listeners=0 class). Never invents free ports.
fn probe_local_desync(
    ticks_since_probe: &mut u32,
    local_port: &AtomicU16,
    e2e: bool,
    daemon_http_port: u16,
) -> bool {
    *ticks_since_probe = ticks_since_probe.wrapping_add(1);
    if *ticks_since_probe < LOCAL_TARGET_PROBE_EVERY {
        return false;
    }
    *ticks_since_probe = 0;
    let port = local_port.load(Ordering::SeqCst);
    if port == 0 || local_port_reachable(port) {
        return false;
    }
    // ensure_https_port (daemon hook) must re-bind if the accept loop died;
    // re_resolve never invents free ports on its own.
    let live = re_resolve_live_local_port(e2e, daemon_http_port, port);
    if !local_port_reachable(live) {
        crate::log_debug!(
            "[tunnel] WARN: local target 127.0.0.1:{port} unreachable and ensure \
             did not restore a live E2E listener (got {live}) — will retry next probe"
        );
        return false;
    }
    if live == port {
        // Frozen port is accepting again (transient blip) — no frpc rewrite.
        return false;
    }
    crate::log_debug!(
        "[tunnel] WARN: local target 127.0.0.1:{port} unreachable; live listener is \
         {live} — self-healing frpc localPort (Bug B desync / E2E respawn)"
    );
    // Stash the resolved live port so the outcome handler rewrites without
    // a second ensure call that could race.
    local_port.store(live, Ordering::SeqCst);
    true
}

/// Before respawning frpc after an exit, ensure the E2E listener is still
/// up (or re-bound) and rewrite frpc.toml if the live port moved. Closes
/// the luzz gap: frpc exit → restart while E2E sockets are gone and the
/// old localPort is frozen dead.
fn ensure_live_local_port_before_respawn(
    e2e: bool,
    daemon_http_port: u16,
    port_slot: &AtomicU16,
    cfg: &TunnelConfig,
    relay: &RelayEndpoint,
) {
    if !e2e {
        return;
    }
    let frozen = port_slot.load(Ordering::SeqCst);
    let live = re_resolve_live_local_port(true, daemon_http_port, frozen);
    if live == 0 {
        return;
    }
    if live != frozen {
        crate::log_debug!(
            "[tunnel] pre-respawn: E2E port moved {frozen} → {live}; rewriting frpc.toml"
        );
        port_slot.store(live, Ordering::SeqCst);
        let _ = super::tls::publish_https_port(live);
        if let Err(e) = write_relay_config(cfg, relay, live, e2e) {
            crate::log_debug!("[tunnel] WARN: pre-respawn rewrite frpc.toml failed: {e}");
        }
        return;
    }
    if !local_port_reachable(live) {
        crate::log_debug!(
            "[tunnel] WARN: pre-respawn: E2E localPort {live} still unreachable after ensure"
        );
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
/// supervisor re-renders `frpc.toml` for the new relay before respawning,
/// resets the backoff, and republishes `current_relay` so `status()`
/// reports the relay actually being dialed. With a single relay the
/// selector never switches; the wait path still polls for stop (so solo
/// frpc is not orphaned) but has no watchdog/fail-back (see
/// [`wait_for_exit`]).
///
/// **Mid-session relay death** (multi-relay only): exits alone can't see a
/// relay that dies AFTER a successful login — frpc never exits then, it
/// reconnect-retries internally forever. Each child therefore gets a
/// [`SessionWatch`] fed by its log pumps; when the [`DisconnectTracker`]
/// declares the session dead the poll loop kills the child
/// ([`ChildOutcome::WatchdogKill`]) and the death is counted as one
/// explicit `on_failure()` — the same threshold/rotation policy as fast
/// dial failures, just triggered by log evidence instead of an exit.
///
/// **Port freeze (R1)**: `resolved_local_port` is captured once at start
/// and reused on every respawn / relay rewrite — a mid-flight daemon port
/// change must not rebind frpc to a different localPort while the public
/// proxy name stays the same. The **only** exception is
/// [`ChildOutcome::LocalPortUnreachable`] self-heal, which re-resolves
/// the **live** E2E port (never a freshly-picked free port).
fn spawn_supervised(
    frpc: PathBuf,
    child_slot: Arc<Mutex<Option<Child>>>,
    running: Arc<AtomicBool>,
    current_relay: Arc<Mutex<RelayEndpoint>>,
    cfg: &TunnelConfig,
    resolved_local_port: Arc<AtomicU16>,
    daemon_http_port: u16,
    e2e: bool,
) -> Result<(), String> {
    // Mid-session watchdog only in multi-relay mode: it exists to drive
    // ROTATION. Solo still polls for stop (orphan fix) but never arms a
    // SessionWatch (nowhere to rotate). One SessionWatch per spawned
    // child so every multi run starts with a clean disconnect streak.
    let multi = cfg.relay_list().len() > 1;
    let new_watch = move || multi.then(|| Arc::new(SessionWatch::new()));

    // First spawn happens synchronously so `start()` fails loud if the
    // very first launch can't even exec.
    let mut watch = new_watch();
    let first = spawn_once(&frpc, watch.clone())?;
    *child_slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(first);

    let frpc_thread = frpc.clone();
    let cfg_thread = cfg.clone();
    let port_slot = resolved_local_port;
    std::thread::Builder::new()
        .name("k2so-frpc-supervisor".to_string())
        .spawn(move || {
            let initial_backoff = Duration::from_millis(500);
            let max_backoff = Duration::from_secs(30);
            let mut backoff = initial_backoff;
            let mut selector = RelaySelector::new(cfg_thread.relay_list());
            // Republish the selector's current relay into the shared slot
            // status() reads — called after every rotation decision so the
            // reported endpoint tracks the selector (the render source of
            // truth) even across a failed frpc.toml rewrite.
            let publish_relay = |relay: &RelayEndpoint| {
                *current_relay.lock().unwrap_or_else(|p| p.into_inner()) = relay.clone();
            };
            // Frozen localPort for every normal rewrite (R1). Self-heal
            // may update `port_slot` then re-read via this helper.
            let frozen_port = || port_slot.load(Ordering::SeqCst);
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
                        // ON-DISK gate at the spawn site (PRD tunnel-
                        // disable-unpair): re-read fresh every attempt —
                        // a disable/release landing mid-flight must stop
                        // the class of respawns, terminally (no backoff
                        // retry-loop from a refused device).
                        if let Err(block) = spawn_gate() {
                            crate::log_debug!(
                                "[tunnel] respawn blocked — supervisor exiting: {block}"
                            );
                            running.store(false, Ordering::SeqCst);
                            return;
                        }
                        watch = new_watch();
                        match spawn_once(&frpc_thread, watch.clone()) {
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
                let outcome = wait_for_exit(
                    &mut child,
                    &running,
                    &mut selector,
                    spawned_at,
                    watch.as_deref(),
                    &port_slot,
                    e2e,
                    daemon_http_port,
                );
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
                            frozen_port(),
                            e2e,
                        ) {
                            crate::log_debug!(
                                "[tunnel] WARN: rewrite frpc.toml for {} failed: {e}",
                                selector.current()
                            );
                        }
                        publish_relay(selector.current());
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
                                frozen_port(),
                                e2e,
                            ) {
                                crate::log_debug!(
                                    "[tunnel] WARN: rewrite frpc.toml for {} failed: {e}",
                                    selector.current()
                                );
                            }
                            publish_relay(selector.current());
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
                        // luzz class: E2E listener can die in the same cascade
                        // that killed frpc. Re-ensure + rewrite BEFORE respawn
                        // so we never point a new frpc at listeners=0.
                        ensure_live_local_port_before_respawn(
                            e2e,
                            daemon_http_port,
                            &port_slot,
                            &cfg_thread,
                            selector.current(),
                        );
                    }
                    ChildOutcome::WatchdogKill => {
                        // The watchdog killed a live-but-stuck child whose
                        // relay died AFTER login. Uptime classification
                        // would credit the long pre-death run as a success,
                        // so the failure is recorded EXPLICITLY: each
                        // declared death is one on_failure(), and the
                        // consecutive-failure threshold still owns the
                        // rotation decision (a lone mid-session death whose
                        // relay answers the redial never rotates — no
                        // thrash). WatchdogKill is never a fail-back:
                        // on_failure() only ever returns rotations.
                        if let Some(sw) = selector.on_failure() {
                            crate::log_debug!(
                                "[tunnel] relay failover: {} -> {} after {} failures",
                                sw.from,
                                sw.to,
                                sw.after_failures
                            );
                            if let Err(e) = write_relay_config(
                                &cfg_thread,
                                selector.current(),
                                frozen_port(),
                                e2e,
                            ) {
                                crate::log_debug!(
                                    "[tunnel] WARN: rewrite frpc.toml for {} failed: {e}",
                                    selector.current()
                                );
                            }
                            publish_relay(selector.current());
                            // Fresh target — dial it promptly instead of
                            // inheriting the dead relay's grown backoff.
                            backoff = initial_backoff;
                        }
                        crate::log_debug!(
                            "[tunnel] frpc cycled by watchdog; restarting in {:?}",
                            backoff
                        );
                        std::thread::sleep(backoff);
                        backoff = (backoff * 2).min(max_backoff);
                    }
                    ChildOutcome::LocalPortUnreachable => {
                        // Bug B self-heal: probe already stashed the live
                        // port into `port_slot`. Rewrite frpc.toml and
                        // respawn frpc only — do NOT kill daemon / PTYs.
                        let live = frozen_port();
                        crate::log_debug!(
                            "[tunnel] self-heal: rewriting frpc.toml localPort={live} \
                             from live E2E listener; restarting frpc child only"
                        );
                        // Re-publish so tunnel-https.port cannot stay stale.
                        if e2e {
                            let _ = super::tls::publish_https_port(live);
                        }
                        if let Err(e) = write_relay_config(
                            &cfg_thread,
                            selector.current(),
                            live,
                            e2e,
                        ) {
                            crate::log_debug!(
                                "[tunnel] WARN: self-heal rewrite frpc.toml failed: {e}"
                            );
                        }
                        if let Err(msg) = tunnel_port_invariant_ok(
                            e2e,
                            if e2e { Some(live) } else { None },
                            super::tls::read_https_port(),
                            parse_local_port_from_frpc_toml(
                                &std::fs::read_to_string(frpc_config_path())
                                    .unwrap_or_default(),
                            ),
                            Some(live),
                        ) {
                            crate::log_debug!(
                                "[tunnel] WARN: port invariant after self-heal: {msg}"
                            );
                        }
                        // Prompt respawn — desync recovery should be fast.
                        backoff = initial_backoff;
                    }
                }
                if !running.load(Ordering::SeqCst) {
                    return;
                }
                // Same ON-DISK gate before the post-exit respawn: read at
                // frpc-spawn time, never from a cached copy (PRD tunnel-
                // disable-unpair pre-mortem). Terminal on block.
                if let Err(block) = spawn_gate() {
                    crate::log_debug!(
                        "[tunnel] respawn blocked — supervisor exiting: {block}"
                    );
                    running.store(false, Ordering::SeqCst);
                    return;
                }
                watch = new_watch();
                match spawn_once(&frpc_thread, watch.clone()) {
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
    if crate::airgap::enabled() {
        crate::log_debug!(
            "[tunnel/lease] air-gap is on (K2_AIRGAP=1) — skipping lease renewal"
        );
        return;
    }
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
    if crate::airgap::enabled() {
        crate::log_debug!(
            "[tunnel/subdomains] air-gap is on (K2_AIRGAP=1) — skipping GET /subdomains"
        );
        return;
    }
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
/// into the append-mode log. When a `watch` is given (multi-relay mode),
/// every line is ALSO fed to the mid-session watchdog — a tee, so the
/// `~/.k2/frpc.log` contract is untouched. Returns the child handle.
fn spawn_once(frpc: &Path, watch: Option<Arc<SessionWatch>>) -> Result<Child, String> {
    let cfg_path = frpc_config_path();
    let log = open_log()?;
    let log_err = log
        .try_clone()
        .map_err(|e| format!("clone log handle: {e}"))?;
    let mut cmd = Command::new(frpc);
    cmd.arg("-c")
        .arg(&cfg_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // frpc is a console-subsystem binary; without this, each tunnel start
    // flashes (or leaves) a black cmd window on the Windows desktop.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn frpc ({}): {e}", frpc.display()))?;

    // Pump stdout/stderr to the log on detached threads so the pipes
    // never fill and block the child. We do NOT log the rendered config
    // or token — only frpc's own output (which never echoes the meta).
    if let Some(out) = child.stdout.take() {
        pump(out, log, watch.clone());
    }
    if let Some(err) = child.stderr.take() {
        pump(err, log_err, watch);
    }
    Ok(child)
}

fn pump(
    reader: impl std::io::Read + Send + 'static,
    mut sink: std::fs::File,
    watch: Option<Arc<SessionWatch>>,
) {
    use std::io::Write;
    std::thread::spawn(move || {
        let buf = BufReader::new(reader);
        for line in buf.lines() {
            match line {
                Ok(l) => {
                    let _ = writeln!(sink, "{l}");
                    if let Some(w) = &watch {
                        w.observe_line(&l);
                    }
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

    /// Mid-session relay death, end to end through the REAL supervise
    /// loop: a fake frpc that LOGS IN successfully and then spews
    /// `connect to server error` lines forever WITHOUT EXITING — exactly
    /// the frpc-internal-reconnect-loop state a post-login relay death
    /// leaves behind, where exit-based failover alone would hang on the
    /// dead relay for good. The watchdog must declare each run dead, kill
    /// the stuck child, and after 3 consecutive watchdog kills the
    /// supervisor must rotate frpc.toml to the SECOND relay.
    #[test]
    fn watchdog_kills_stuck_midsession_child_and_supervisor_rotates() {
        with_temp_home(|| {
            // Fake frpc: one success line, then an error storm, never exits.
            // The storm rate (10 lines/s) crosses the watchdog threshold
            // (3 in 30s) almost immediately.
            let dir = std::env::temp_dir().join(format!(
                "k2so-fake-frpc-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).expect("mk fake-frpc dir");
            let script = dir.join("frpc-stuck.sh");
            std::fs::write(
                &script,
                "#!/bin/sh\n\
                 echo '[I] [service.go:299] login to server success, get run id [test]'\n\
                 while true; do\n\
                 \techo '[W] [service.go:132] connect to server error: dial tcp 10.0.0.1:7000: i/o timeout'\n\
                 \tsleep 0.1\n\
                 done\n",
            )
            .expect("write fake frpc");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                    .expect("chmod fake frpc");
            }

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

            let st = start(None, 57839, &FrpcBinary::Explicit(script.clone()))
                .expect("start with stuck fake frpc");
            assert!(st.running);
            let toml = std::fs::read_to_string(frpc_config_path()).expect("frpc.toml");
            assert!(
                toml.contains("serverAddr = \"10.0.0.1\""),
                "must dial the primary first\n{toml}"
            );

            // Each cycle: watchdog declares dead within ~1 poll tick (the
            // error storm crosses the threshold in ~0.3s), kill, backoff
            // (0.5s/1s/2s), respawn. Three cycles land well inside the
            // deadline; poll generously so slow CI can't flake this.
            let deadline = Instant::now() + Duration::from_secs(60);
            let rotated = loop {
                let toml = std::fs::read_to_string(frpc_config_path()).expect("frpc.toml");
                if toml.contains("serverAddr = \"10.0.0.2\"") {
                    break toml;
                }
                if Instant::now() >= deadline {
                    panic!(
                        "watchdog never converted the stuck child into a rotation; \
                         frpc.toml:\n{toml}"
                    );
                }
                std::thread::sleep(Duration::from_millis(200));
            };

            // Only the server endpoint rotates — token/subdomain/proxy name
            // are byte-identical (the relays are peers).
            assert!(rotated.contains("token = \"tok\""), "{rotated}");
            assert!(rotated.contains("subdomain = \"rosson\""), "{rotated}");
            assert!(rotated.contains("name = \"k2so-rosson\""), "{rotated}");

            // The tee contract: the stuck child's lines still landed in
            // ~/.k2/frpc.log — the watchdog reads a COPY, not the file.
            let log = std::fs::read_to_string(frpc_log_path()).expect("frpc.log");
            assert!(
                log.contains("login to server success"),
                "frpc.log must still capture child output\n{log}"
            );
            assert!(log.contains("connect to server error"), "{log}");

            stop().expect("stop");
            assert!(!status().running);
            let _ = std::fs::remove_dir_all(&dir);
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
            // Status keeps reporting the legacy endpoint too — the live-
            // relay slot never rotates on a solo config, so the reported
            // server_addr is byte-identical to the pre-failover status.
            assert_eq!(
                status().server_addr.as_deref(),
                Some("9.9.9.9"),
                "single-relay status must report the configured endpoint"
            );
            let _ = stop();
        });
    }

    /// TunnelStatus reports the LIVE relay, not the configured primary:
    /// through the REAL supervise loop, a two-relay config whose "frpc"
    /// (/bin/true) fails fast must — after the 3-failure rotation — flip
    /// `status().server_addr` to the SECOND relay's host, while the rest
    /// of the status (URL, subdomain) is untouched by the rotation.
    #[test]
    fn status_reports_live_relay_after_failover_rotation() {
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
            // Before any rotation the status homes to the PRIMARY.
            assert_eq!(
                st.server_addr.as_deref(),
                Some("10.0.0.1"),
                "fresh start must report the preferred relay"
            );

            // /bin/true exits immediately → 3 fast failures rotate to the
            // second relay within a few seconds (see the frpc.toml twin of
            // this test); poll generously so slow CI can't flake this.
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let now = status();
                if now.server_addr.as_deref() == Some("10.0.0.2") {
                    // Only the relay changed — the public identity of the
                    // tunnel is untouched by a rotation.
                    assert!(now.running, "rotation must not read as a stop");
                    assert_eq!(now.public_url.as_deref(), Some("https://rosson.k2.dev"));
                    assert_eq!(now.subdomain.as_deref(), Some("rosson"));
                    break;
                }
                if Instant::now() >= deadline {
                    panic!(
                        "status never reported the rotated relay; still {:?}",
                        now.server_addr
                    );
                }
                std::thread::sleep(Duration::from_millis(200));
            }

            stop().expect("stop");
            assert!(!status().running);
        });
    }

    /// Setting `relays` through the UPDATE path (`set_config`) must not
    /// disturb a currently-connected tunnel — exactly like server_addr,
    /// it's persist-only, picked up by the NEXT start. Guards the "no new
    /// restart semantic" contract end to end: running tunnel keeps its
    /// endpoint + rendered frpc.toml; a stop/start dials the new primary.
    #[test]
    fn set_relays_while_running_applies_only_on_next_start() {
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

            let st = start(None, 57839, &FrpcBinary::Explicit(true_bin()))
                .expect("start on legacy endpoint");
            assert!(st.running);

            // Mutate the relay list through the same surface the daemon's
            // POST /cli/tunnel/config uses, while the tunnel is up.
            super::super::set_config(super::super::TunnelConfigUpdate {
                relays: Some(vec![
                    RelayEndpoint { host: "10.0.0.1".to_string(), port: 7000 },
                    RelayEndpoint { host: "10.0.0.2".to_string(), port: 7000 },
                ]),
                ..Default::default()
            })
            .expect("set relays while running");

            // The RUNNING tunnel is undisturbed: still up, still homed to
            // the endpoint it started on, rendered config untouched.
            let now = status();
            assert!(now.running, "set_config must not stop a running tunnel");
            assert_eq!(
                now.server_addr.as_deref(),
                Some("9.9.9.9"),
                "live tunnel must keep its endpoint until the next start"
            );
            let toml = std::fs::read_to_string(frpc_config_path()).expect("frpc.toml");
            assert!(
                toml.contains("serverAddr = \"9.9.9.9\""),
                "set_config must not re-render the live frpc.toml\n{toml}"
            );

            // The NEXT start picks the new list up: dials the new primary.
            stop().expect("stop");
            let st = start(None, 57839, &FrpcBinary::Explicit(true_bin()))
                .expect("restart with relays");
            assert_eq!(
                st.server_addr.as_deref(),
                Some("10.0.0.1"),
                "restart must home to the new preferred relay"
            );
            let toml = std::fs::read_to_string(frpc_config_path()).expect("frpc.toml");
            assert!(
                toml.contains("serverAddr = \"10.0.0.1\""),
                "restart must render the new preferred relay\n{toml}"
            );
            stop().expect("stop");
        });
    }

    /// PRD tunnel-disable-unpair §2A — DISABLE BLOCKS SPAWN: with the
    /// persisted pause flag down, `start()` must refuse before any side
    /// effect (no frpc.toml render, no child), whoever calls it (boot
    /// autostart, route, CLI — all funnel here).
    #[test]
    fn start_refuses_when_disabled_and_never_renders() {
        with_temp_home(|| {
            config::save(&TunnelConfig {
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                e2e: false,
                enabled: false,
                ..Default::default()
            })
            .expect("seed disabled config");

            let err = start(None, 57839, &FrpcBinary::Explicit(true_bin()))
                .expect_err("disabled tunnel must refuse to start");
            assert!(
                err.contains("disabled"),
                "error must name the persisted disable, got: {err}"
            );
            assert!(!status().running, "no child after a refused start");
            assert!(
                !frpc_config_path().exists(),
                "a refused start must not render frpc.toml (gate sits ABOVE the connector)"
            );
            // Status carries the tri-state truth for the UI.
            let st = status();
            assert!(!st.enabled, "status must report the persisted disable");
            assert!(!st.released);

            // Re-enable is symmetric: same config, flag up, start works.
            config::update(|c| c.enabled = true).expect("re-enable");
            let st = start(None, 57839, &FrpcBinary::Explicit(true_bin()))
                .expect("re-enabled tunnel must start");
            assert!(st.running);
            stop().expect("stop");
        });
    }

    /// PRD tunnel-disable-unpair §2B — a RELEASED identity can never
    /// re-arm the connector: a planted copy of the old tunnel.json (the
    /// stale-backup / zombie case) is refused by the local gate even
    /// before the relay gets a say.
    #[test]
    fn released_identity_cannot_rearm_connector() {
        with_temp_home(|| {
            // The planted stale backup: a fully connectable config…
            config::save(&TunnelConfig {
                token: "tok-old".to_string(),
                subdomain: "rosson".to_string(),
                e2e: false,
                ..Default::default()
            })
            .expect("plant stale identity");
            // …whose identity was RELEASED on this device.
            super::super::unpair::save(&super::super::unpair::UnpairTombstone {
                released_at: "2026-07-12T00:00:00+00:00".to_string(),
                subdomain: "rosson".to_string(),
                device_id: None,
                token_sha256: super::super::unpair::token_fingerprint("tok-old"),
                upstream_reported: true,
                pending_token: None,
            })
            .expect("save tombstone");

            let err = start(None, 57839, &FrpcBinary::Explicit(true_bin()))
                .expect_err("released identity must never re-arm");
            assert!(
                err.contains("released"),
                "error must name the release, got: {err}"
            );
            assert!(!status().running);
            assert!(status().released, "status must report the released state");

            // A FRESH identity (normal re-pair) passes the gate and starts.
            config::save(&TunnelConfig {
                token: "tok-new".to_string(),
                subdomain: "rosson".to_string(),
                e2e: false,
                ..Default::default()
            })
            .expect("re-pair with fresh identity");
            let st = start(None, 57839, &FrpcBinary::Explicit(true_bin()))
                .expect("fresh identity must start");
            assert!(st.running);
            stop().expect("stop");
        });
    }

    /// The incident's KILL-THE-CLASS requirement, end to end through the
    /// REAL supervisor: a disable persisted ON DISK while the tunnel is up
    /// must stop the respawn loop at the next child exit — the supervisor
    /// re-reads the flag at spawn time (no cached copy), exits terminally,
    /// and flips the connector to stopped. This is exactly what an
    /// orphaned daemon does after the user disables from anywhere else.
    #[test]
    fn supervisor_stops_respawning_after_on_disk_disable() {
        with_temp_home(|| {
            config::save(&TunnelConfig {
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                e2e: false,
                ..Default::default()
            })
            .expect("seed config");

            // /bin/true exits instantly → the supervisor is in a steady
            // exit→backoff→respawn cycle.
            let st = start(None, 57839, &FrpcBinary::Explicit(true_bin()))
                .expect("start");
            assert!(st.running);

            // The disable lands ON DISK only — no stop() call, exactly
            // like a second daemon / another surface flipping the flag.
            config::update(|c| c.enabled = false).expect("persist disable");

            // The next respawn attempt must observe the flag and exit the
            // supervisor terminally. Backoff starts at 0.5s; poll
            // generously so slow CI can't flake this.
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                if !status().running {
                    break;
                }
                if Instant::now() >= deadline {
                    panic!("supervisor kept respawning after an on-disk disable");
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            let st = status();
            assert!(!st.enabled, "status must show the persisted disable");
            stop().expect("stop is idempotent after the gate exit");
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

    /// stop() always reaps by config pattern even when the Child slot is
    /// empty (supervisor took the handle) — the orphan-kill path that
    /// used to only run on start(). Spawns a long-lived stand-in whose
    /// cmdline matches `stray_frpc_pattern`, never installs connector
    /// STATE, then stop() must kill it.
    #[cfg(unix)]
    #[test]
    fn stop_reaps_stray_frpc_even_when_child_slot_empty() {
        with_temp_home(|| {
            let cfg_path = frpc_config_path();
            if let Some(dir) = cfg_path.parent() {
                std::fs::create_dir_all(dir).expect("mk .k2");
            }
            std::fs::write(&cfg_path, "# orphan-reap test config\n").expect("write frpc.toml");

            // Binary MUST be named `frpc` so the full cmdline contains the
            // pkill pattern `frpc -c <cfg>` (same as a real frpc spawn).
            let dir = std::env::temp_dir().join(format!(
                "k2so-orphan-frpc-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).expect("mk orphan-frpc dir");
            let frpc = dir.join("frpc");
            std::fs::write(&frpc, "#!/bin/sh\nsleep 300\n").expect("write sleep-frpc");
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&frpc, std::fs::Permissions::from_mode(0o755))
                    .expect("chmod sleep-frpc");
            }

            let mut child = Command::new(&frpc)
                .arg("-c")
                .arg(&cfg_path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn orphan stand-in");
            let pid = child.id();
            // Let the process table settle so pkill -f can see it.
            std::thread::sleep(Duration::from_millis(150));
            // Sanity: still running before stop (not yet exited).
            assert!(
                child.try_wait().expect("try_wait before").is_none(),
                "orphan stand-in (pid {pid}) must be alive before stop"
            );

            // No connector STATE — slot is empty by construction.
            stop().expect("stop must succeed with empty STATE");
            assert!(!status().running);

            // pkill SIGTERMs the stand-in; the Child handle reaps the
            // zombie. Do NOT use `kill -0` — zombies still "exist" and
            // would false-pass as alive.
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut gone = false;
            while Instant::now() < deadline {
                match child.try_wait() {
                    Ok(Some(_)) => {
                        gone = true;
                        break;
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                    Err(_) => {
                        gone = true;
                        break;
                    }
                }
            }
            assert!(
                gone,
                "stop() must reap the stray frpc (pid {pid}) matching {}",
                stray_frpc_pattern(&cfg_path)
            );
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// Solo-mode stop must kill a long-lived supervised child (the
    /// wait_for_exit poll observes `running=false`). Before the fix, solo
    /// blocked on bare `child.wait()` and stop only killed an empty slot.
    #[cfg(unix)]
    #[test]
    fn solo_stop_kills_long_lived_supervised_child() {
        with_temp_home(|| {
            let dir = std::env::temp_dir().join(format!(
                "k2so-solo-stop-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).expect("mk solo-stop dir");
            // Named `frpc` so stop()'s pattern reap is a second line of defense.
            let script = dir.join("frpc");
            std::fs::write(&script, "#!/bin/sh\nsleep 300\n").expect("write long-lived frpc");
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                    .expect("chmod long-lived frpc");
            }

            config::save(&TunnelConfig {
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                e2e: false,
                ..Default::default()
            })
            .expect("seed solo config");

            let st = start(None, 57839, &FrpcBinary::Explicit(script.clone()))
                .expect("start long-lived solo frpc");
            assert!(st.running, "solo start must report running");
            // Give the supervisor a tick to .take() the Child so the slot
            // is empty — the historical bug path.
            std::thread::sleep(Duration::from_millis(200));

            stop().expect("stop solo tunnel");
            assert!(!status().running, "status must be stopped after stop()");

            // Child must not still be holding the config path (reap + poll kill).
            // pgrep -f: exit 0 = match still alive, exit 1 = no match (desired).
            // Fail loudly if pgrep is missing or cannot run — never soft-pass.
            let pattern = stray_frpc_pattern(&frpc_config_path());
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut clean = false;
            while Instant::now() < deadline {
                let status = Command::new("pgrep")
                    .arg("-f")
                    .arg(&pattern)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .expect("pgrep -f must be available to assert solo stop killed frpc");
                // Unix: 0 = found, 1 = not found. Anything else is a tool error.
                let code = status.code().expect("pgrep must exit with a status code");
                assert!(
                    code == 0 || code == 1,
                    "pgrep -f unexpected exit {code} for pattern `{pattern}`"
                );
                if code == 1 {
                    clean = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            assert!(
                clean,
                "solo stop must leave no process matching `{pattern}`"
            );
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// R1 port freeze: `resolved_local_port` is captured once at start and
    /// reused on every multi-relay rewrite — rotation must not change
    /// localPort even as serverAddr flips.
    #[test]
    fn supervisor_rotation_keeps_frozen_local_port() {
        with_temp_home(|| {
            config::save(&TunnelConfig {
                relays: vec![
                    RelayEndpoint {
                        host: "10.0.0.1".to_string(),
                        port: 7000,
                    },
                    RelayEndpoint {
                        host: "10.0.0.2".to_string(),
                        port: 7000,
                    },
                ],
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                e2e: false,
                ..Default::default()
            })
            .expect("seed two-relay config");

            // Pin an explicit non-default daemon port so a freeze bug would
            // be obvious (would rebind to 0 / missing / wrong port).
            let st = start(None, 48123, &FrpcBinary::Explicit(true_bin()))
                .expect("start with two relays");
            assert!(st.running);
            assert_eq!(
                st.local_port,
                Some(48123),
                "start must freeze the resolved daemon port into status"
            );
            let first = std::fs::read_to_string(frpc_config_path()).expect("frpc.toml");
            assert!(
                first.contains("localPort = 48123"),
                "initial render must use the frozen port\n{first}"
            );

            let deadline = Instant::now() + Duration::from_secs(30);
            let rotated = loop {
                let toml = std::fs::read_to_string(frpc_config_path()).expect("frpc.toml");
                if toml.contains("serverAddr = \"10.0.0.2\"") {
                    break toml;
                }
                if Instant::now() >= deadline {
                    panic!("supervisor never rotated; frpc.toml:\n{toml}");
                }
                std::thread::sleep(Duration::from_millis(200));
            };
            assert!(
                rotated.contains("localPort = 48123"),
                "relay rotation must keep the frozen localPort from start\n{rotated}"
            );
            assert_eq!(
                status().local_port,
                Some(48123),
                "status must still report the frozen port after rotation"
            );
            // Invariant: frpc localPort still matches frozen after rotation.
            tunnel_port_invariant_ok(
                false,
                None,
                None,
                parse_local_port_from_frpc_toml(&rotated),
                Some(48123),
            )
            .expect("port invariant must hold after relay rotation");

            stop().expect("stop");
        });
    }

    #[test]
    fn parse_local_port_from_frpc_toml_reads_renderer_shape() {
        assert_eq!(
            parse_local_port_from_frpc_toml("localPort = 44407\n"),
            Some(44407)
        );
        assert_eq!(
            parse_local_port_from_frpc_toml("  localPort = 42265  \n"),
            Some(42265)
        );
        assert_eq!(parse_local_port_from_frpc_toml("serverAddr = \"x\"\n"), None);
        assert_eq!(parse_local_port_from_frpc_toml("localPort = 0\n"), None);
    }

    #[test]
    fn tunnel_port_invariant_ok_detects_e2e_desync() {
        // Happy path: live == published == frpc == frozen.
        tunnel_port_invariant_ok(true, Some(44407), Some(44407), Some(44407), Some(44407))
            .expect("aligned ports must pass");
        // luzz class: live 44407 vs frpc/file 42265.
        let err = tunnel_port_invariant_ok(
            true,
            Some(44407),
            Some(42265),
            Some(42265),
            Some(42265),
        )
        .expect_err("live vs published desync must fail");
        assert!(
            err.contains("44407") && err.contains("42265"),
            "error must name both ports: {err}"
        );
        let err = tunnel_port_invariant_ok(
            true,
            Some(44407),
            Some(44407),
            Some(42265),
            Some(42265),
        )
        .expect_err("frpc localPort desync must fail");
        assert!(
            err.contains("42265") && err.contains("44407"),
            "error must name both ports: {err}"
        );
        // E2E off: only frpc vs frozen matter.
        tunnel_port_invariant_ok(false, Some(1), Some(2), Some(48123), Some(48123))
            .expect("E2E off ignores live/published HTTPS mismatch");
        let err =
            tunnel_port_invariant_ok(false, None, None, Some(1), Some(2)).expect_err("mismatch");
        assert!(err.contains("E2E off"), "got: {err}");
    }

    /// Pure re-resolve path: when E2E is on and only the published port
    /// file is available (no daemon hook), re_resolve returns the file —
    /// never invents a free port.
    #[test]
    fn re_resolve_live_local_port_uses_published_not_invented() {
        with_temp_home(|| {
            super::super::tls::publish_https_port(44407).expect("publish");
            let got = re_resolve_live_local_port(true, 9999, 42265);
            assert_eq!(
                got, 44407,
                "must prefer published live port over stale freeze {got}"
            );
            // E2E off: freeze is sticky (no re-pick).
            assert_eq!(re_resolve_live_local_port(false, 9999, 42265), 42265);
        });
    }

    /// When the frozen localPort is dead and ensure cannot restore a live
    /// listener, probe must NOT thrash (return false). After the daemon hook
    /// re-binds (live port differs), probe returns true so frpc rewrites.
    #[test]
    fn probe_local_desync_false_when_no_live_listener() {
        let port = AtomicU16::new(39999); // almost certainly closed
        let mut ticks = LOCAL_TARGET_PROBE_EVERY;
        // e2e=false → re_resolve returns current; still unreachable → false.
        assert!(
            !probe_local_desync(&mut ticks, &port, false, 18080),
            "must not self-heal when no live alternate port exists"
        );
        assert_eq!(port.load(Ordering::SeqCst), 39999);
    }

    #[test]
    fn local_port_reachable_false_for_closed_port() {
        // Bind then drop so we know a free port that is not listening.
        let port = {
            let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
            l.local_addr().expect("addr").port()
        };
        assert!(
            !local_port_reachable(port),
            "closed port {port} must report unreachable"
        );
        assert!(!local_port_reachable(0), "port 0 is never reachable");
    }

    #[test]
    fn local_port_reachable_true_for_open_listener() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().expect("addr").port();
        // Accept in background so connect completes (some platforms need it).
        let _accept = std::thread::spawn(move || {
            let _ = listener.accept();
        });
        assert!(
            local_port_reachable(port),
            "open listener on {port} must report reachable"
        );
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
