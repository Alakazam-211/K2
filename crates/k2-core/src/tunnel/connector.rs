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

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::config::{self, RelayEndpoint, TunnelConfig, SUBDOMAIN_HOST};
use super::failover::{
    DnsHint, FailoverMode, FailoverStatus, FailureDecision, RelaySelector, RelaySwitch,
};
use super::frpc_proc::{self, FrpcProc};
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
    // On macOS the daemon runs from `Contents/Helpers/K2 Daemon.app`, so
    // resolve the HOST app's `Contents/MacOS`, not the helper's.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = crate::daemon_lifecycle::bundle_sidecar_dir(&exe) {
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
    let path = crate::terminal::path_env::lookup_path()?;
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
///
/// K2 vault (§10.4): outside tests it lives in a private runtime
/// directory (Linux the daemon unit's `/run/k2`, else `$XDG_RUNTIME_DIR/k2`
/// or `/dev/shm/k2-<uid>`; macOS the per-user temp dir; see
/// [`crate::runtime_dir`]), not in `~/.k2`, so home backups and clones
/// never carry the rendered token.
pub(crate) fn frpc_config_path() -> PathBuf {
    #[cfg(not(test))]
    {
        if let Some(d) = crate::runtime_dir::private_runtime_dir() {
            return d.join("frpc.toml");
        }
    }
    legacy_frpc_config_path()
}

/// The tunnel's own files in a runtime dir.
#[cfg(all(unix, not(test)))]
const RUNTIME_DIR_FILES: &[&str] = &["frpc.toml", "frpc.pid"];

/// 0.46.4 move onto the daemon unit's `RuntimeDirectory=` (akzm,
/// 2026-10-24): an frpc the previous daemon left running from an older
/// runtime dir (`/run/user/<uid>/k2`, `/dev/shm/k2-<uid>`) still holds the
/// `k2so-<sub>` proxy, so it is reaped before the fresh spawn (else frps
/// answers "proxy already exists"), and the tunnel's files there go
/// ([`crate::runtime_dir::clean_superseded_dir`]: K2's names only, no
/// symlinks). Idempotent, so "once" = the first start after the move.
/// Never under `cfg(test)`, the `K2_RUNTIME_DIR` override or a test
/// process ([`crate::runtime_dir::superseded_runtime_dirs`]), and not when
/// the config is on the `~/.k2` fallback (no runtime dir at all).
fn reap_superseded_runtime_dirs(current_cfg: &Path) {
    #[cfg(all(unix, not(test)))]
    {
        if current_cfg == legacy_frpc_config_path() {
            return;
        }
        let Some(current) = current_cfg.parent() else { return };
        for dir in crate::runtime_dir::superseded_runtime_dirs(current) {
            // By argv, not by file: logind may already have deleted the
            // config a live frpc was started with.
            let old_cfg = dir.join("frpc.toml");
            let was_running = stray_frpc_alive(&old_cfg);
            if was_running {
                reap_stray_frpc(&old_cfg);
            }
            let removed = crate::runtime_dir::clean_superseded_dir(&dir, RUNTIME_DIR_FILES);
            if was_running || !removed.is_empty() {
                crate::log_debug!(
                    "[tunnel] runtime dir is now {}: {} the frpc started from {}; removed {:?}",
                    current.display(),
                    if was_running { "reaped" } else { "no" },
                    dir.display(),
                    removed
                );
            }
        }
    }
    #[cfg(any(test, not(unix)))]
    let _ = current_cfg;
}

/// The pre-vault location (`~/.k2/frpc.toml`): reaped and removed.
pub(crate) fn legacy_frpc_config_path() -> PathBuf {
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

/// Whether frps accepted THIS run's proxy (0.45.2). A running frpc is not
/// proof that this device holds the name: a stale copy of a migrated box's
/// identity also runs frpc, but frps refuses its proxy ("proxy already
/// exists") because the live box registered it first. The pump threads feed
/// every frpc log line here; frpc prefixes each proxy line with the proxy
/// name in brackets (`[k2so-<sub>] start proxy success`).
///
/// Every spawn starts a new run ([`Self::begin_run`]) and clears the flag, so
/// a respawn or a relay rotation must register again. A late line from a
/// previous child is ignored (its run number is stale). A later
/// `start error` for our proxy (frpc re-logged in and the name was taken)
/// clears it.
/// frpc log fragments that mean the control session to frps is gone (or
/// being re-established), so our proxy registration no longer holds.
/// Strings from frp 0.61's client (`client/service.go`, `client/control.go`)
/// and from this box's own frpc.log.
const SESSION_LOST_MARKERS: &[&str] = &[
    "try to connect to server",
    "connect to server error",
    "login to server failed",
    "try to reconnect",
    "work connection closed",
    "session shutdown",
];

pub(crate) struct ProxyRegistration {
    proxy_name: String,
    /// The current child's run number (0 = never spawned).
    run: AtomicU64,
    /// The run whose log said `start proxy success` (0 = none).
    registered_run: AtomicU64,
}

impl ProxyRegistration {
    pub(crate) fn new(cfg: &TunnelConfig) -> Self {
        let sub = cfg.subdomain.trim();
        let proxy_name = if sub.is_empty() { "k2so-daemon".to_string() } else { format!("k2so-{sub}") };
        Self { proxy_name, run: AtomicU64::new(0), registered_run: AtomicU64::new(0) }
    }

    /// A child is about to be spawned: a new run, not registered.
    pub(crate) fn begin_run(&self) -> u64 {
        let run = self.run.fetch_add(1, Ordering::SeqCst) + 1;
        self.registered_run.store(0, Ordering::SeqCst);
        run
    }

    /// Whether `run` is the current child (not a previous child's late output).
    pub(crate) fn is_current_run(&self, run: u64) -> bool {
        self.run.load(Ordering::SeqCst) == run
    }

    /// One frpc log line from the child of `run`.
    pub(crate) fn observe_line(&self, run: u64, line: &str) {
        if !self.is_current_run(run) {
            return; // a previous child's late output
        }
        if line.contains("login to server success") {
            FRPC_LOGINS.fetch_add(1, Ordering::SeqCst);
        }
        // The control session to frps went away (frp 0.61 lines, e.g.
        // `[W] [client/service.go:319] [<runid>] connect to server error: …`,
        // `[I] [client/service.go:308] [<runid>] try to connect to server...`,
        // `[W] [client/control.go:119] [<runid>] start new connection to
        // server error: session shutdown`). frps drops our proxies with the
        // session; we hold the name again only after a new success line.
        if SESSION_LOST_MARKERS.iter().any(|m| line.contains(m)) {
            let _ = self
                .registered_run
                .compare_exchange(run, 0, Ordering::SeqCst, Ordering::SeqCst);
            return;
        }
        if !line.contains(&format!("[{}]", self.proxy_name)) {
            return;
        }
        if line.contains("start proxy success") {
            self.registered_run.store(run, Ordering::SeqCst);
        } else if line.contains("start error") || line.contains("already exists") {
            let _ = self
                .registered_run
                .compare_exchange(run, 0, Ordering::SeqCst, Ordering::SeqCst);
        }
    }

    /// frps accepted the current run's proxy.
    pub(crate) fn is_registered(&self) -> bool {
        let run = self.run.load(Ordering::SeqCst);
        run != 0 && self.registered_run.load(Ordering::SeqCst) == run
    }
}

/// How the supervised frpc is ACTUALLY doing (2026-10-14 incident). The
/// connector's `running` flag is the desired state; it stayed true for two
/// hours while frpc crash-looped on a missing config, so `k2 tunnel
/// status` said "Running: yes" and `k2 tunnel start` said "started".
///
/// A failure is a run that ended before it was up (no `start proxy
/// success` and under [`HEALTHY_UPTIME`]), a watchdog kill, or a spawn /
/// config-render error. A run that comes up resets the count.
pub(crate) struct FrpcHealth {
    consecutive_failures: AtomicU64,
    /// The last frpc error line, spawn error or render error.
    last_error: Mutex<Option<String>>,
}

/// frpc log fragments worth surfacing as the last error.
const ERROR_LINE_MARKERS: &[&str] = &[" [E] ", "[E] [", "no such file", "error", "failed"];

impl FrpcHealth {
    pub(crate) fn new() -> Self {
        Self { consecutive_failures: AtomicU64::new(0), last_error: Mutex::new(None) }
    }

    fn set_error(&self, why: impl Into<String>) {
        let mut why: String = why.into();
        if why.len() > 400 {
            let mut cut = 400;
            while !why.is_char_boundary(cut) {
                cut -= 1;
            }
            why.truncate(cut);
        }
        *self.last_error.lock().unwrap_or_else(|p| p.into_inner()) = Some(why);
    }

    /// One failed run / spawn / render.
    pub(crate) fn record_failure(&self, why: impl Into<String>) {
        self.consecutive_failures.fetch_add(1, Ordering::SeqCst);
        let why = why.into();
        // Keep frpc's own error line when the run already logged one; the
        // exit code alone says less.
        let mut slot = self.last_error.lock().unwrap_or_else(|p| p.into_inner());
        if slot.is_none() {
            *slot = Some(why);
        }
    }

    /// One failed spawn / render: `why` replaces the last error.
    pub(crate) fn fail_with(&self, why: impl Into<String>) {
        self.consecutive_failures.fetch_add(1, Ordering::SeqCst);
        self.set_error(why);
    }

    /// The current run came up.
    pub(crate) fn record_up(&self) {
        if self.consecutive_failures.swap(0, Ordering::SeqCst) != 0 {
            crate::log_debug!("[tunnel] frpc is up again");
        }
        *self.last_error.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    /// One frpc log line (from the pump threads).
    pub(crate) fn observe_line(&self, line: &str) {
        let lower = line.to_ascii_lowercase();
        if ERROR_LINE_MARKERS.iter().any(|m| lower.contains(&m.to_ascii_lowercase())) {
            self.set_error(line.trim().to_string());
        }
    }

    pub(crate) fn failures(&self) -> u64 {
        self.consecutive_failures.load(Ordering::SeqCst)
    }

    pub(crate) fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

/// Live connector state — the supervised child + the desired-state flag.
struct ConnectorState {
    /// The currently-running config.
    cfg: TunnelConfig,
    /// frpc `localPort` — frozen at start (R1) and only rewritten by the
    /// self-heal path when the frozen target is unreachable. Shared so
    /// `status()` and the supervisor always agree after a heal.
    resolved_local_port: Arc<AtomicU16>,
    /// The frpc process (our child, or one adopted from the previous
    /// daemon — runtime stage H). `None` between restarts.
    child: Arc<Mutex<Option<FrpcProc>>>,
    /// Desired state: `true` = should be running (supervisor restarts on
    /// exit); `false` = stop requested (supervisor must not restart).
    running: Arc<AtomicBool>,
    /// The relay the supervisor is CURRENTLY homed to — index 0 of the
    /// fallback list at start, republished by the supervise loop on every
    /// failover/fail-back so `status()` reports the LIVE relay rather than
    /// the configured primary. A single-relay config never rotates, so
    /// this stays the legacy endpoint and status is unchanged.
    current_relay: Arc<Mutex<RelayEndpoint>>,
    /// Whether `current_relay` is the primary (index 0 of the list the
    /// supervisor runs), republished with it (0.45.7).
    relay_primary: Arc<AtomicBool>,
    /// 0.46.3: the failover state (rules + what they are doing), published
    /// by the supervise loop with every decision. `None` for one relay.
    failover: Arc<Mutex<Option<FailoverStatus>>>,
    /// Whether frps accepted the current child's proxy (holder evidence
    /// for a release).
    proxy: Arc<ProxyRegistration>,
    /// Whether frpc is actually up (vs crash-looping).
    health: Arc<FrpcHealth>,
}

static STATE: OnceLock<Mutex<Option<ConnectorState>>> = OnceLock::new();

/// Runtime stage H: this daemon handed off to a successor, which adopts
/// the running frpc. Every supervisor then lets go of its process WITHOUT
/// killing it ([`detach_for_handoff`]).
static DETACHED: AtomicBool = AtomicBool::new(false);

/// Hand-off: stop supervising frpc but leave it running for the successor
/// to adopt (its pid record stays). Lease renewal and the subdomain
/// refresh stop too (the successor starts its own). Returns the frpc pid
/// when one was running. One-way.
pub fn detach_for_handoff() -> Option<i32> {
    DETACHED.store(true, Ordering::SeqCst);
    let guard = state().lock().unwrap_or_else(|p| p.into_inner());
    let st = guard.as_ref()?;
    st.running.store(false, Ordering::SeqCst);
    let pid = frpc_proc::read_pid_record(&frpc_config_path()).map(|r| r.pid);
    pid.filter(|p| frpc_proc::pid_alive(*p))
}

fn detached() -> bool {
    DETACHED.load(Ordering::SeqCst)
}

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
fn spawn_gate() -> Result<TunnelConfig, SpawnBlock> {
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
    // The fresh on-disk config: the supervisor adopts a changed relay
    // list from it at each reconnect (0.45.7).
    Ok(cfg)
}

/// TCP connect budget for one probe of the primary relay.
const PRIMARY_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// How often a SOLO supervisor whose session the watchdog declared dead
/// re-reads `tunnel.json` for a longer relay list (0.45.7). Short in
/// tests so they can wait on several checks.
#[cfg(not(test))]
const SOLO_LIST_CHECK_EVERY: Duration = Duration::from_secs(5);
#[cfg(test)]
const SOLO_LIST_CHECK_EVERY: Duration = Duration::from_millis(250);

/// Solo dead-session list checks that found the list UNCHANGED (and so
/// left frpc's own retry alone). Lets tests wait on the decision itself
/// instead of sleeping.
static SOLO_LIST_UNCHANGED_CHECKS: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
static TEST_FAILBACK_POLICY: Mutex<Option<super::failover::FailbackPolicy>> = Mutex::new(None);

/// Overrides the fail-back timing for supervisors started while it lives
/// (tests only; hold it inside `with_temp_home`).
#[cfg(test)]
pub(crate) struct TestFailbackPolicy;

#[cfg(test)]
impl TestFailbackPolicy {
    pub(crate) fn install(p: super::failover::FailbackPolicy) -> Self {
        *TEST_FAILBACK_POLICY.lock().unwrap_or_else(|e| e.into_inner()) = Some(p);
        TestFailbackPolicy
    }
}

#[cfg(test)]
impl Drop for TestFailbackPolicy {
    fn drop(&mut self) {
        *TEST_FAILBACK_POLICY.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

#[cfg(test)]
static TEST_DNS_MIN_OUTAGE: Mutex<Option<Duration>> = Mutex::new(None);

/// Overrides the DNS-mode minimum outage (150 s) for supervisors started
/// while it lives (tests only; hold it inside `with_temp_home`).
#[cfg(test)]
pub(crate) struct TestDnsMinOutage;

#[cfg(test)]
impl TestDnsMinOutage {
    pub(crate) fn install(d: Duration) -> Self {
        *TEST_DNS_MIN_OUTAGE.lock().unwrap_or_else(|e| e.into_inner()) = Some(d);
        TestDnsMinOutage
    }
}

#[cfg(test)]
impl Drop for TestDnsMinOutage {
    fn drop(&mut self) {
        *TEST_DNS_MIN_OUTAGE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

fn failback_policy(mode: FailoverMode) -> super::failover::FailbackPolicy {
    #[cfg(test)]
    {
        if let Some(p) = *TEST_FAILBACK_POLICY.lock().unwrap_or_else(|e| e.into_inner()) {
            return p;
        }
    }
    match mode {
        FailoverMode::Legacy => super::failover::FailbackPolicy::DEFAULT,
        FailoverMode::Dns => super::failover::FailbackPolicy::DNS_AWARE,
    }
}

#[cfg(test)]
static TEST_REACHABLE: Mutex<Option<fn(&RelayEndpoint) -> bool>> = Mutex::new(None);

/// Overrides the standby reachability probe for supervisors running while
/// it lives (tests only; hold it inside `with_temp_home`).
#[cfg(test)]
pub(crate) struct TestReachability;

#[cfg(test)]
impl TestReachability {
    pub(crate) fn install(f: fn(&RelayEndpoint) -> bool) -> Self {
        *TEST_REACHABLE.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
        TestReachability
    }
}

#[cfg(test)]
impl Drop for TestReachability {
    fn drop(&mut self) {
        *TEST_REACHABLE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// 0.46.3: whether a relay we are about to move to answers (TCP connect to
/// its frps port, [`probe_relay`]).
fn relay_reachable(relay: &RelayEndpoint) -> bool {
    #[cfg(test)]
    {
        if let Some(f) = *TEST_REACHABLE.lock().unwrap_or_else(|e| e.into_inner()) {
            return f(relay);
        }
    }
    probe_relay(relay)
}

fn dns_min_outage() -> Duration {
    #[cfg(test)]
    {
        if let Some(d) = *TEST_DNS_MIN_OUTAGE.lock().unwrap_or_else(|e| e.into_inner()) {
            return d;
        }
    }
    super::failover::DEFAULT_DNS_MIN_OUTAGE
}

/// The selector for a tunnel start. [`FailoverMode::Legacy`] builds the
/// 0.46.2 selector exactly; [`FailoverMode::Dns`] adds the DNS gate and the
/// faster fail-back.
fn new_selector(relays: Vec<RelayEndpoint>, mode: FailoverMode) -> RelaySelector {
    let sel = RelaySelector::with_policy(
        relays,
        super::failover::DEFAULT_FAILURE_THRESHOLD,
        failback_policy(mode),
    );
    match mode {
        FailoverMode::Legacy => sel,
        FailoverMode::Dns => sel.with_dns_gate(dns_min_outage()),
    }
}

/// 0.46.3: where the supervise loop gets k2.dev's word on which relay
/// `*.k2.dev` points at. Never blocks: a fresh list is fetched on the
/// relay-list refresher thread ([`super::relay_list::request_refresh`],
/// at most every [`super::relay_list::DNS_HINT_REQUEST_EVERY`]) while the
/// last known answer is used meanwhile. In legacy mode it never asks and
/// always says [`DnsHint::Unknown`] (which the legacy rules ignore).
struct HintSource {
    mode: FailoverMode,
    last_request: Option<Instant>,
}

impl HintSource {
    fn new(mode: FailoverMode) -> Self {
        Self {
            mode,
            last_request: None,
        }
    }

    /// For a decision: ask for a fresh answer (async, rate-limited) and
    /// return the last known one.
    fn decide(&mut self) -> DnsHint {
        if self.mode == FailoverMode::Legacy {
            return DnsHint::Unknown;
        }
        let due = self
            .last_request
            .map_or(true, |t| t.elapsed() >= super::relay_list::DNS_HINT_REQUEST_EVERY);
        if due {
            self.last_request = Some(Instant::now());
            super::relay_list::request_refresh("failover check");
        }
        self.cached()
    }

    /// The last known answer, no request.
    fn cached(&self) -> DnsHint {
        if self.mode == FailoverMode::Legacy {
            return DnsHint::Unknown;
        }
        super::relay_list::dns_hint(super::relay_list::DNS_HINT_MAX_AGE)
    }
}

/// Feed one failure to the selector under its rules and log a hold or
/// (DNS mode) why it moves. The caller logs the move itself, with the
/// same line as 0.46.2.
fn decide_failure(selector: &mut RelaySelector, hints: &mut HintSource) -> Option<RelaySwitch> {
    let hint = hints.decide();
    // DNS mode probes a standby before moving to it (never the primary);
    // legacy never calls this.
    let mut reachable = |r: &RelayEndpoint| {
        let ok = relay_reachable(r);
        if !ok {
            crate::log_debug!("[tunnel] failover: standby {r} probe: unreachable");
        }
        ok
    };
    match selector.on_failure_at(Instant::now(), &hint, &mut reachable) {
        FailureDecision::Count => None,
        FailureDecision::Hold(why) => {
            crate::log_debug!("[tunnel] failover: {why}");
            None
        }
        FailureDecision::Switch(sw, why) => {
            if selector.mode() == FailoverMode::Dns {
                crate::log_debug!("[tunnel] failover: moving to {}: {why}", sw.to);
            }
            Some(sw)
        }
    }
}

fn publish_failover(slot: &Mutex<Option<FailoverStatus>>, sel: &RelaySelector) {
    *slot.lock().unwrap_or_else(|p| p.into_inner()) = sel.failover_status();
}

/// One health probe of a relay: a plain TCP connect to its frps port.
/// Healthy = some resolved address accepted within the budget.
fn probe_relay(relay: &RelayEndpoint) -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    let addrs = match (relay.host.as_str(), relay.port).to_socket_addrs() {
        Ok(a) => a.collect::<Vec<_>>(),
        Err(_) => return false,
    };
    addrs
        .iter()
        .any(|a| TcpStream::connect_timeout(a, PRIMARY_PROBE_TIMEOUT).is_ok())
}

/// The on-disk relay list differs from the one the supervisor runs.
fn disk_relays_differ(selector: &RelaySelector) -> bool {
    match config::load() {
        Ok(c) => c.relay_list() != selector.relays(),
        Err(_) => false,
    }
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
    /// Running only: `false` when frpc is crash-looping or its rendered
    /// config is missing — `running` is only the desired state (2026-10-14:
    /// "Running: yes" for two hours while every frpc start failed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frpc_healthy: Option<bool>,
    /// Running only: frpc runs in a row that failed before coming up.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frpc_consecutive_failures: Option<u64>,
    /// Running only: the last frpc error line / spawn or render error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frpc_last_error: Option<String>,
    /// Running only: whether the rendered frpc config is on disk.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frpc_config_present: Option<bool>,
    /// Where the rendered frpc config lives (private runtime dir).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frpc_config: Option<String>,
    /// Set by a `start` that found the tunnel "running" but frpc failing
    /// (or its config gone) and repaired it: the reason it repaired.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repaired: Option<String>,
    /// Running only (0.45.7): the frps port of the relay in use
    /// (`server_addr` is its host).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_port: Option<u16>,
    /// Running only (0.45.7): whether the relay in use is the primary
    /// (index 0). `false` = failed over; the supervisor probes the primary
    /// and returns after 3 healthy probes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay_primary: Option<bool>,
    /// Running with 2+ relays only (0.46.3): the failover rules in use
    /// (`dns` / `legacy`) and what they are doing (e.g. waiting for DNS).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failover: Option<FailoverStatus>,
    /// The relay list on disk: where it came from (`fetched` /
    /// `provisioned` / `default`), its order, and the last k2.dev refresh.
    /// Always emitted.
    pub relay_list: super::relay_list::RelayListStatus,
}

impl TunnelStatus {
    fn stopped() -> Self {
        // Disk truth for the tri-state: the connector singleton being
        // empty says nothing about WHY the tunnel is down.
        let cfg = config::load().unwrap_or_default();
        let relay_list = super::relay_list::status_for(&cfg);
        Self {
            running: false,
            public_url: None,
            subdomain: None,
            local_port: None,
            server_addr: None,
            frpc_installed: resolve_frpc(&FrpcBinary::Auto).is_ok(),
            enabled: cfg.enabled,
            released: super::unpair::released_state(&cfg),
            frpc_healthy: None,
            frpc_consecutive_failures: None,
            frpc_last_error: None,
            frpc_config_present: None,
            frpc_config: Some(frpc_config_path().display().to_string()),
            repaired: None,
            server_port: None,
            relay_primary: None,
            failover: None,
            relay_list,
        }
    }
}

/// Why a "running" connector is not actually serving (`None` = healthy):
/// the rendered config is gone, or frpc runs keep failing.
fn unhealthy_reason(st: &ConnectorState) -> Option<String> {
    let path = frpc_config_path();
    if !path.exists() {
        return Some(format!("frpc config missing at {}", path.display()));
    }
    let n = st.health.failures();
    if n > 0 {
        let err = st.health.last_error().unwrap_or_else(|| "no error line".to_string());
        return Some(format!("frpc failing ({n} consecutive failures): {err}"));
    }
    None
}

/// Prefix of the [`start`] error for a vault-held token that could not be
/// read. A soft failure: nothing was stopped or cleared; boot auto-start
/// matches it to retry later.
pub const TOKEN_UNREADABLE_PREFIX: &str = "tunnel token unreadable";

fn token_unreadable_error(e: &str) -> String {
    format!(
        "{TOKEN_UNREADABLE_PREFIX}: the K2 vault could not be read ({e}); the stored \
         token is kept and nothing was changed — try again once the vault is readable"
    )
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
    // A daemon that handed off never starts frpc: the successor adopts it.
    crate::handoff_state::guard()?;
    let mut guard = state().lock().unwrap_or_else(|p| p.into_inner());
    // Set when a "running" tunnel was found broken and restarted below.
    let mut repaired: Option<String> = None;

    // Idempotent: already supervising a live child — for the SAME tunnel
    // identity. A live child for another identity (the subdomain/token on
    // disk or in `subdomain_override` moved since it started) is restarted
    // under the new one; returning the old status would keep frpc serving
    // the old name (0.45.2: release + connect a new subdomain).
    //
    // Same identity but NOT serving (frpc crash-looping, or the rendered
    // config gone — 2026-10-14) is repaired the same way: stop + a fresh
    // start re-renders the config and respawns frpc. "Tunnel started" over
    // a dead frpc was the bug.
    //
    // Pause-safe (0.45.7, grokbot): a start that lands while frpc is alive
    // never restarts it for a reason frpc can ride out on its own. A
    // machine that was paused or asleep comes back with frpc reconnecting
    // by itself (and the supervisor + watchdog handle a dead one), so:
    //   * frps has the live run's proxy registered → never restart, even
    //     when the rendered config file went missing (frpc read it at
    //     spawn; it is re-rendered here, in place, and before any respawn);
    //   * the config on disk can't start a tunnel at all (no token — e.g.
    //     the vault read failed right after a wake) → keep the live
    //     tunnel; stopping it would only trade a working tunnel for an
    //     error.
    if let Some(st) = guard.as_ref() {
        if st.running.load(Ordering::SeqCst) {
            let mut desired = config::load()?;
            if let Some(sub) = subdomain_override.as_deref() {
                desired.subdomain = sub.to_string();
            }
            // The vault-held token could not be read (0.45.7): the identity
            // on disk is unknown, not changed. Keep the live tunnel; the
            // next start re-reads the vault.
            if let Some(e) = desired.token_read_error.as_deref() {
                crate::log_debug!(
                    "[tunnel] start: the tunnel token could not be read from the K2 vault \
                     ({e}) — keeping the running tunnel for {:?} up",
                    st.cfg.subdomain
                );
                return Ok(status_from(st));
            }
            let stop_reason;
            if super::identity::TunnelIdentity::of(&desired)
                == super::identity::TunnelIdentity::of(&st.cfg)
            {
                match unhealthy_reason(st) {
                    None => return Ok(status_from(st)),
                    Some(why) if st.proxy.is_registered() => {
                        crate::log_debug!(
                            "[tunnel] start: {why}, but frps has the live frpc's proxy \
                             registered — leaving frpc up and re-rendering the config in place"
                        );
                        rerender_in_place(st);
                        return Ok(status_from(st));
                    }
                    Some(why) => {
                        crate::log_debug!(
                            "[tunnel] start: tunnel is running but not serving ({why}) — \
                             re-rendering the config and restarting frpc"
                        );
                        stop_reason = format!("start: repair, running but not serving ({why})");
                        repaired = Some(why);
                    }
                }
            } else if !desired.is_connectable() {
                crate::log_debug!(
                    "[tunnel] start: the config on disk cannot start a tunnel (no token or \
                     server; the vault may be unreadable) — keeping the running tunnel for \
                     {:?} up instead of restarting it into a failure",
                    st.cfg.subdomain
                );
                return Ok(status_from(st));
            } else {
                crate::log_debug!(
                    "[tunnel] start: running tunnel serves {:?} but the config now says {:?} \
                     — restarting under the new identity",
                    st.cfg.subdomain,
                    desired.subdomain
                );
                stop_reason = format!(
                    "start: identity change, running {:?} but the config says {:?}",
                    st.cfg.subdomain, desired.subdomain
                );
            }
            drop(guard);
            stop(&stop_reason)?;
            guard = state().lock().unwrap_or_else(|p| p.into_inner());
            // A concurrent start may have won the slot while it was free.
            if let Some(st) = guard.as_ref() {
                if st.running.load(Ordering::SeqCst) {
                    return Ok(status_from(st));
                }
            }
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
    if let Some(e) = cfg.token_read_error.as_deref() {
        return Err(token_unreadable_error(e));
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
    // PRD connect-login-edge-only I3: with E2E OFF, cleartext tunnel bytes
    // must still land on the daemon's TUNNEL-INGRESS listener (dispatch
    // tagged `Ingress::Tunnel`), never the privileged main loopback
    // listener. Resolved through the daemon-registered seam; loud `Err`
    // on bind failure (no silent fallback to the main port).
    //
    // Persist FIRST (0.45.2): the ensure hook reads the subdomain from disk
    // to name the cert and the listener, so a `subdomain_override` must be
    // on disk before it runs — otherwise `start?subdomain=new` brought the
    // listener up for the OLD name.
    let mut to_save = cfg.clone();
    to_save.local_port = None;
    config::save(&to_save)?;
    let resolved_local_port = if e2e {
        super::tls::ensure_https_port(default_local_port)?
    } else {
        super::ingress::ensure_tunnel_ingress_port(default_local_port)?
    };

    // Resolve frpc + render config to disk (0600). `e2e` selects the proxy
    // type (https vs http) and was used above to pick `resolved_local_port`.
    // Always dial the PREFERRED relay first (index 0 of the fallback list);
    // for a legacy single-endpoint config that IS `server_addr`/`server_port`,
    // so the rendered TOML is byte-identical to the pre-failover path.
    let frpc = resolve_frpc(bin)?;
    let rendered = render_frpc_toml_for_relay(&cfg, &cfg.relay_list()[0], resolved_local_port, e2e);

    // Runtime stage H (AR3/K37): adopt the frpc the previous daemon left
    // running (hand-off, crash, `KillMode=process` restart) when it still
    // runs exactly this config — no relay re-dial, no tunnel blip. Any
    // mismatch falls through to the reap + fresh spawn below.
    let adopted = match frpc_proc::try_adopt(&frpc_config_path(), &rendered) {
        Ok(rec) => {
            crate::log_debug!(
                "[tunnel] adopting running frpc pid {} (same config, localPort {resolved_local_port})",
                rec.pid
            );
            Some(rec)
        }
        Err(frpc_proc::NoAdopt::NoRecord) => None,
        Err(why) => {
            crate::log_debug!("[tunnel] not adopting the recorded frpc ({why:?}) — fresh spawn");
            None
        }
    };
    if adopted.is_none() {
        write_config_file(&rendered)?;
    }

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
    // (guard returned early otherwise), so every match is genuinely stale
    // — except the one frpc we just adopted, which is left alone.
    if adopted.is_none() {
        reap_stray_frpc(&frpc_config_path());
        frpc_proc::clear_pid_record(&frpc_config_path());
    }
    let legacy = legacy_frpc_config_path();
    if legacy != frpc_config_path() {
        reap_stray_frpc(&legacy);
        let _ = std::fs::remove_file(&legacy);
    }
    reap_superseded_runtime_dirs(&frpc_config_path());

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
    let relay_primary = Arc::new(AtomicBool::new(true));
    let proxy = Arc::new(ProxyRegistration::new(&cfg));
    let health = Arc::new(FrpcHealth::new());
    // 0.46.3: the failover rules, read once per start (tunnel.json
    // `failover`, K2_TUNNEL_FAILOVER wins).
    let failover_mode = config::failover_mode(&cfg);
    if let Some(raw) = cfg.failover.as_deref() {
        if FailoverMode::parse(raw).is_none() {
            crate::log_debug!(
                "[tunnel] WARN: tunnel.json failover={raw:?} is not \"dns\" or \"legacy\"; using {}",
                failover_mode.as_str()
            );
        }
    }
    if cfg.relay_list().len() > 1 {
        crate::log_debug!("[tunnel] relay failover rules: {}", failover_mode.as_str());
    }
    let failover = Arc::new(Mutex::new(
        new_selector(cfg.relay_list(), failover_mode).failover_status(),
    ));
    spawn_supervised(
        frpc,
        child.clone(),
        running.clone(),
        current_relay.clone(),
        relay_primary.clone(),
        failover.clone(),
        failover_mode,
        &cfg,
        resolved_port_slot.clone(),
        default_local_port,
        e2e,
        proxy.clone(),
        health.clone(),
        adopted,
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
        relay_primary,
        failover,
        proxy,
        health,
    };
    let mut status = status_from(&st);
    status.repaired = repaired;
    *guard = Some(st);
    SPAWN_GENERATION.fetch_add(1, Ordering::SeqCst);
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
///
/// `reason` names the caller and why (0.45.7): every stop is logged with
/// it, so a stop in the field is never anonymous again.
pub fn stop(reason: &str) -> Result<(), String> {
    let mut guard = state().lock().unwrap_or_else(|p| p.into_inner());
    let live = guard.as_ref().filter(|st| st.running.load(Ordering::SeqCst));
    crate::log_debug!(
        "{}",
        stop_log_line(
            reason,
            live.map(|st| st.cfg.subdomain.as_str()),
            live.is_some_and(|st| st.proxy.is_registered()),
        )
    );
    *last_stop_reason_slot().lock().unwrap_or_else(|p| p.into_inner()) = Some(reason.to_string());
    if let Some(st) = guard.as_ref() {
        // 1) Tell the supervisor not to respawn — MUST be first.
        st.running.store(false, Ordering::SeqCst);
        // 2) Kill the Child handle if the supervisor hasn't taken it yet.
        if let Some(child) = st.child.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
            // Best-effort kill. frpc has no special signal protocol.
            child.kill_and_wait();
        }
    }
    // A stopped tunnel leaves nothing to adopt.
    frpc_proc::clear_pid_record(&frpc_config_path());
    // 3) Always reap by config pattern — independent of Child ownership.
    //    Covers: supervisor-held child (slot empty), orphans from prior
    //    crashes, and solo-mode waits that never observed `running=false`
    //    before this fix. Safe when nothing matches (pkill no-match = ok).
    let cfg_path = frpc_config_path();
    crate::log_debug!(
        "[tunnel] stop ({reason}): reaping stray frpc matching `{}`",
        stray_frpc_pattern(&cfg_path)
    );
    reap_stray_frpc(&cfg_path);
    if legacy_frpc_config_path() != cfg_path {
        reap_stray_frpc(&legacy_frpc_config_path());
    }
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

/// The `[tunnel] stop` log line: who/why, and what was running. Pure so
/// the wording is unit-tested; never carries a token.
fn stop_log_line(reason: &str, running_sub: Option<&str>, registered: bool) -> String {
    match running_sub {
        Some(sub) => format!(
            "[tunnel] stop ({reason}): stopping the tunnel for {sub:?} (frps proxy registered: {registered})"
        ),
        None => format!("[tunnel] stop ({reason}): no tunnel running; reaping any stray frpc"),
    }
}

fn last_stop_reason_slot() -> &'static Mutex<Option<String>> {
    static SLOT: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// The reason given to the most recent [`stop`] (`None` = never stopped
/// in this process).
pub fn last_stop_reason() -> Option<String> {
    last_stop_reason_slot().lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// Fresh frpc supervisors started by [`start`] in this process. A start
/// that found the tunnel already serving does not bump it, so equal
/// values before and after a call prove frpc was not restarted.
static SPAWN_GENERATION: AtomicU64 = AtomicU64::new(0);

/// frpc logins to frps accepted since this daemon started (the
/// `login to server success` line). The relay learns the box's public IP
/// at Login, so Sew voice retries a TURN DNS `403 ip_unverified` after the
/// count moves (k2-connect `docs/turn.md`).
static FRPC_LOGINS: AtomicU64 = AtomicU64::new(0);

/// See [`FRPC_LOGINS`].
pub fn frpc_login_count() -> u64 {
    FRPC_LOGINS.load(Ordering::SeqCst)
}

pub fn spawn_generation() -> u64 {
    SPAWN_GENERATION.load(Ordering::SeqCst)
}

/// Re-render a missing frpc.toml for the LIVE run without touching frpc
/// (it read the file at spawn). Same inputs the supervisor uses.
fn rerender_in_place(st: &ConnectorState) {
    if frpc_config_path().exists() {
        return;
    }
    let relay = st.current_relay.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let port = st.resolved_local_port.load(Ordering::SeqCst);
    if let Err(e) = write_relay_config(&st.cfg, &relay, port, config::e2e_enabled(&st.cfg)) {
        crate::log_debug!("[tunnel] WARN: in-place re-render of frpc.toml failed: {e}");
    }
}

/// The config the RUNNING connector started with (`None` when stopped).
/// The tunnel-identity path compares it with the config on disk.
pub fn running_config() -> Option<TunnelConfig> {
    let guard = state().lock().unwrap_or_else(|p| p.into_inner());
    match guard.as_ref() {
        Some(st) if st.running.load(Ordering::SeqCst) => Some(st.cfg.clone()),
        _ => None,
    }
}

/// The config the running connector started with, ONLY when frps also
/// accepted its proxy for the current frpc run ([`ProxyRegistration`]).
/// This is the holder evidence a release needs: frpc running alone is not.
pub fn registered_config() -> Option<TunnelConfig> {
    let guard = state().lock().unwrap_or_else(|p| p.into_inner());
    match guard.as_ref() {
        Some(st) if st.running.load(Ordering::SeqCst) && st.proxy.is_registered() => {
            Some(st.cfg.clone())
        }
        _ => None,
    }
}

/// Whether frps accepted the running connector's proxy (false when stopped).
pub fn proxy_registered() -> bool {
    registered_config().is_some()
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
    let relay = st
        .current_relay
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    let server_addr = relay.host.clone();
    // The list status reads the file (the refresher may have written a
    // newer list than the running one); fall back to the running config.
    let relay_list =
        super::relay_list::status_for(&config::load().unwrap_or_else(|_| st.cfg.clone()));
    let cfg_path = frpc_config_path();
    let unhealthy = unhealthy_reason(st);
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
        frpc_healthy: Some(unhealthy.is_none()),
        frpc_consecutive_failures: Some(st.health.failures()),
        frpc_last_error: st.health.last_error().or(unhealthy),
        frpc_config_present: Some(cfg_path.exists()),
        frpc_config: Some(cfg_path.display().to_string()),
        repaired: None,
        server_port: Some(relay.port),
        relay_primary: Some(st.relay_primary.load(Ordering::SeqCst)),
        failover: st
            .failover
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone(),
        relay_list,
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
    /// Runtime stage H: the daemon handed off; the child keeps running for
    /// the successor to adopt. The supervisor exits without a kill.
    Detached,
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

/// Per-child-run health bookkeeping inside [`wait_for_exit`]: marks the
/// run up (frps accepted the proxy, or it outlived [`HEALTHY_UPTIME`]) and
/// re-renders the config whenever it has gone missing (macOS purges
/// untouched files in the per-user temp dir after ~3 days; a test daemon
/// once deleted it) so the NEXT respawn never execs frpc against nothing.
struct RunHealth<'a> {
    proxy: &'a ProxyRegistration,
    health: &'a FrpcHealth,
    /// Re-render the config if it is missing (closure over the current
    /// relay + frozen port).
    ensure_config: &'a dyn Fn(),
    came_up: bool,
    ticks: u32,
}

impl RunHealth<'_> {
    fn tick(&mut self, spawned_at: Instant) {
        if !self.came_up
            && (self.proxy.is_registered() || spawned_at.elapsed() >= HEALTHY_UPTIME)
        {
            self.came_up = true;
            self.health.record_up();
        }
        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks % LOCAL_TARGET_PROBE_EVERY == 0 {
            (self.ensure_config)();
        }
    }
}

/// Wait for the current child to end.
///
/// * Single relay ([`RelaySelector::is_solo`]) — poll `try_wait` so we can
///   (0.45.7: a dead session is cycled ONLY when `tunnel.json` now lists
///   other relays, so a refreshed list reaches it; see the solo loop.)
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
    child: &mut FrpcProc,
    running: &AtomicBool,
    selector: &mut RelaySelector,
    hints: &mut HintSource,
    failover_slot: &Mutex<Option<FailoverStatus>>,
    spawned_at: Instant,
    watch: Option<&SessionWatch>,
    local_port: &AtomicU16,
    e2e: bool,
    daemon_http_port: u16,
    run: &mut RunHealth<'_>,
) -> ChildOutcome {
    let mut ticks_since_probe = 0u32;
    let mut published_up = false;
    // Solo: stop-observability + localPort self-heal — no fail-back. The
    // watchdog only acts when the session is dead AND `tunnel.json` now
    // lists other relays (0.45.7: a refreshed list must reach a box whose
    // only relay died mid-session — frpc would retry it forever).
    if selector.is_solo() {
        let mut last_list_check: Option<Instant> = None;
        loop {
            run.tick(spawned_at);
            match child.try_wait() {
                Ok(Some(code)) => return ChildOutcome::Exited(code),
                Ok(None) => {}
                Err(e) => {
                    crate::log_debug!("[tunnel] wait on frpc child failed: {e}");
                    return ChildOutcome::Exited(None);
                }
            }
            if !running.load(Ordering::SeqCst) {
                if detached() {
                    return ChildOutcome::Detached;
                }
                child.kill_and_wait();
                return ChildOutcome::Exited(None);
            }
            if let Some(w) = watch {
                let check_due =
                    last_list_check.map_or(true, |t| t.elapsed() >= SOLO_LIST_CHECK_EVERY);
                if w.is_dead() && check_due {
                    last_list_check = Some(Instant::now());
                    if disk_relays_differ(selector) {
                        crate::log_debug!(
                            "[tunnel] relay {} session is dead and tunnel.json lists other \
                             relays — reconnecting to apply the new list",
                            selector.current()
                        );
                        child.kill_and_wait();
                        return ChildOutcome::WatchdogKill;
                    }
                    SOLO_LIST_UNCHANGED_CHECKS.fetch_add(1, Ordering::SeqCst);
                }
            }
            if probe_local_desync(
                &mut ticks_since_probe,
                local_port,
                e2e,
                daemon_http_port,
            ) {
                child.kill_and_wait();
                return ChildOutcome::LocalPortUnreachable;
            }
            std::thread::sleep(SUPERVISE_POLL);
        }
    }
    loop {
        run.tick(spawned_at);
        match child.try_wait() {
            Ok(Some(code)) => return ChildOutcome::Exited(code),
            Ok(None) => {} // still running
            Err(e) => {
                crate::log_debug!("[tunnel] wait on frpc child failed: {e}");
                return ChildOutcome::Exited(None);
            }
        }
        if !running.load(Ordering::SeqCst) {
            // Hand-off: the successor adopts this frpc — let go, no kill.
            if detached() {
                return ChildOutcome::Detached;
            }
            // Stop requested while the child lives: we hold the handle,
            // so the kill is ours to do. The caller returns right after.
            child.kill_and_wait();
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
                child.kill_and_wait();
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
            child.kill_and_wait();
            return ChildOutcome::LocalPortUnreachable;
        }
        // A child alive past HEALTHY_UPTIME is a working relay: reset the
        // failure counter.
        if spawned_at.elapsed() >= HEALTHY_UPTIME {
            selector.on_success();
            // 0.46.3: a hold ("waiting for DNS") ends with a working run.
            if !published_up {
                published_up = true;
                publish_failover(failover_slot, selector);
            }
        }
        // Fail-back (0.45.7): on a fallback, TCP-probe the primary on the
        // selector's schedule; after enough healthy probes in a row (and
        // its random delay) return to it with one controlled reconnect.
        // Never fires while the primary is in use.
        // 0.46.3 DNS mode: not while k2.dev says the name points at the
        // fallback in use (legacy: the hint is always Unknown and the
        // selector runs the 0.46.2 rules).
        if selector.primary_probe_due(Instant::now()) {
            let primary = selector.primary().clone();
            let healthy = probe_relay(&primary);
            let hint = hints.decide();
            selector.on_primary_probe_at(healthy, Instant::now(), &hint);
            crate::log_debug!(
                "[tunnel] primary relay {primary} probe: {} ({} healthy in a row)",
                if healthy { "healthy" } else { "unreachable" },
                selector.probe_streak()
            );
            if selector.mode() == FailoverMode::Dns
                && matches!(&hint, DnsHint::Active(r) if r == selector.current())
            {
                crate::log_debug!(
                    "[tunnel] failover: staying on {}, k2.dev says the name points here",
                    selector.current()
                );
            }
            publish_failover(failover_slot, selector);
        }
        if let Some(sw) = selector.poll_failback_at(Instant::now(), &hints.cached()) {
            child.kill_and_wait();
            return ChildOutcome::FailBack(sw);
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
/// wrapping; or 3 healthy probes of the primary → back to it), the
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
    child_slot: Arc<Mutex<Option<FrpcProc>>>,
    running: Arc<AtomicBool>,
    current_relay: Arc<Mutex<RelayEndpoint>>,
    relay_primary: Arc<AtomicBool>,
    failover_slot: Arc<Mutex<Option<FailoverStatus>>>,
    failover_mode: FailoverMode,
    cfg: &TunnelConfig,
    resolved_local_port: Arc<AtomicU16>,
    daemon_http_port: u16,
    e2e: bool,
    proxy: Arc<ProxyRegistration>,
    health: Arc<FrpcHealth>,
    adopted: Option<frpc_proc::PidRecord>,
) -> Result<(), String> {
    // One SessionWatch per spawned child so every run starts with a clean
    // disconnect streak. Multi-relay: it drives ROTATION. Solo (0.45.7):
    // it only lets a dead session reconnect when `tunnel.json` gained
    // other relays (see `wait_for_exit`); otherwise frpc keeps its own
    // retry, exactly as before.
    let new_watch = || Some(Arc::new(SessionWatch::new()));

    // First spawn happens synchronously so `start()` fails loud if the
    // very first launch can't even exec. Like every spawn, it renders the
    // config first ([`render_and_spawn`]). An adopted frpc (stage H) is
    // supervised exactly like one we spawned (its config is already on
    // disk and unchanged — `try_adopt` checked).
    let mut watch = new_watch();
    let (first, mut tail_stop) = match adopted {
        Some(rec) => adopt_once(&rec, watch.clone(), &proxy, &health),
        None => render_and_spawn(
            &frpc,
            cfg,
            &cfg.relay_list()[0],
            resolved_local_port.load(Ordering::SeqCst),
            e2e,
            watch.clone(),
            &proxy,
            &health,
        )?,
    };
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
            let mut selector = new_selector(cfg_thread.relay_list(), failover_mode);
            let mut hints = HintSource::new(failover_mode);
            // Republish the selector's current relay into the shared slot
            // status() reads — called after every rotation decision so the
            // reported endpoint tracks the selector (the render source of
            // truth) even across a failed frpc.toml rewrite. 0.46.3: the
            // failover state rides along.
            let publish_relay = |sel: &RelaySelector| {
                *current_relay.lock().unwrap_or_else(|p| p.into_inner()) = sel.current().clone();
                relay_primary.store(sel.on_primary(), Ordering::SeqCst);
                publish_failover(&failover_slot, sel);
            };
            // 0.45.7: a relay list that changed on disk (k2.dev refresh,
            // pairing, a manual edit) is applied HERE, at a reconnect —
            // never as a live restart. The relay in use stays in use when
            // the new list still has it.
            let adopt_disk_list = |sel: &mut RelaySelector, disk: &TunnelConfig| {
                let list = disk.relay_list();
                if list.as_slice() == sel.relays() {
                    return;
                }
                let moved = sel.adopt(list);
                crate::log_debug!(
                    "[tunnel] relay list changed on disk — applying it at this reconnect \
                     ({} relay(s); dialing {}{})",
                    sel.relays().len(),
                    sel.current(),
                    if moved { ", the old relay is no longer listed" } else { "" }
                );
            };
            // Frozen localPort for every normal rewrite (R1). Self-heal
            // may update `port_slot` then re-read via this helper.
            let frozen_port = || port_slot.load(Ordering::SeqCst);
            // Re-render the config if it vanished while frpc runs (macOS
            // temp purge, a stray deleter). The relay slot always holds
            // the selector's current relay (republished on every rotation).
            let ensure_config = || {
                let path = frpc_config_path();
                if path.exists() {
                    return;
                }
                let relay = current_relay.lock().unwrap_or_else(|p| p.into_inner()).clone();
                crate::log_debug!(
                    "[tunnel] WARN: frpc config {} is missing while frpc runs — re-rendering",
                    path.display()
                );
                if let Err(e) =
                    write_relay_config(&cfg_thread, &relay, port_slot.load(Ordering::SeqCst), e2e)
                {
                    crate::log_debug!("[tunnel] WARN: re-render of missing frpc config failed: {e}");
                    health.fail_with(format!("re-render missing frpc config: {e}"));
                }
            };
            loop {
                // Take the current child to wait on it, or spawn the next
                // one. The `running` check and the spawn happen under the
                // child-slot lock: `stop()` clears `running` and THEN takes
                // that lock before it reaps, so a respawn is either finished
                // before stop's reap (and reaped with it) or sees `running`
                // false. Without it, a supervisor that passed the check just
                // before a stop spawned one more frpc after the reap: a stray
                // child, and its log lines landed in the NEXT tunnel's tail
                // (a "healthy" tunnel with a dead run's error, ticket
                // a56b93cc).
                let mut slot = child_slot.lock().unwrap_or_else(|p| p.into_inner());
                let mut child = match slot.take() {
                    Some(c) => {
                        drop(slot);
                        c
                    }
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
                        match spawn_gate() {
                            Err(block) => {
                                crate::log_debug!(
                                    "[tunnel] respawn blocked — supervisor exiting: {block}"
                                );
                                running.store(false, Ordering::SeqCst);
                                return;
                            }
                            Ok(disk) => {
                                adopt_disk_list(&mut selector, &disk);
                                publish_relay(&selector);
                            }
                        }
                        watch = new_watch();
                        match render_and_spawn(
                            &frpc_thread,
                            &cfg_thread,
                            selector.current(),
                            frozen_port(),
                            e2e,
                            watch.clone(),
                            &proxy,
                            &health,
                        ) {
                            Ok((c, stop)) => {
                                drop(slot);
                                tail_stop = stop;
                                c
                            }
                            Err(e) => {
                                drop(slot);
                                crate::log_debug!("[tunnel] respawn failed: {e}");
                                std::thread::sleep(backoff);
                                backoff = (backoff * 2).min(max_backoff);
                                continue;
                            }
                        }
                    }
                };

                let spawned_at = Instant::now();
                let mut run = RunHealth {
                    proxy: &proxy,
                    health: &health,
                    ensure_config: &ensure_config,
                    came_up: false,
                    ticks: 0,
                };
                let outcome = wait_for_exit(
                    &mut child,
                    &running,
                    &mut selector,
                    &mut hints,
                    &failover_slot,
                    spawned_at,
                    watch.as_deref(),
                    &port_slot,
                    e2e,
                    daemon_http_port,
                    &mut run,
                );
                // This run's log tail ends with the run (an adopted or
                // detached frpc keeps writing, but not for us).
                tail_stop.store(true, Ordering::SeqCst);
                if matches!(outcome, ChildOutcome::Detached) {
                    crate::log_debug!(
                        "[tunnel] hand-off: leaving frpc pid {} running for the successor",
                        child.pid()
                    );
                    // Never kill or reap it: the successor adopts it.
                    std::mem::forget(child);
                    return;
                }
                let came_up = run.came_up;
                if !running.load(Ordering::SeqCst) {
                    // Stop requested — do not restart.
                    return;
                }
                // Health: a run that never came up is a failure (the
                // 2026-10-14 crash loop: frpc exits at once on a missing
                // config); a watchdog kill always is.
                match &outcome {
                    ChildOutcome::Exited(code) if !came_up => health.record_failure(format!(
                        "frpc exited ({code:?}) after {:.1}s without registering its proxy",
                        spawned_at.elapsed().as_secs_f64()
                    )),
                    ChildOutcome::WatchdogKill => {
                        health.fail_with("relay session dead (mid-session watchdog killed frpc)")
                    }
                    _ => {}
                }
                match outcome {
                    ChildOutcome::Detached => unreachable!("handled above"),
                    ChildOutcome::FailBack(sw) => {
                        // Deliberate switch back to the preferred relay:
                        // re-render, reset backoff, respawn immediately
                        // (no failure happened, so no penalty sleep).
                        crate::log_debug!(
                            "[tunnel] relay fail-back: {} -> {} (primary passed its health probes)",
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
                        publish_relay(&selector);
                        super::relay_list::request_refresh("relay switch");
                        backoff = initial_backoff;
                    }
                    ChildOutcome::Exited(code) => {
                        // Classify the run for the selector: a quick death
                        // is a failed dial on the current relay, a long run
                        // was a working connection (resets the counter; the
                        // return to the primary is probe-driven, never here).
                        let switch = if spawned_at.elapsed() >= HEALTHY_UPTIME {
                            selector.on_success();
                            None
                        } else {
                            // 0.46.3: legacy = exactly on_failure(); DNS
                            // mode may hold on the primary (logged).
                            decide_failure(&mut selector, &mut hints)
                        };
                        publish_failover(&failover_slot, &selector);
                        if let Some(sw) = switch {
                            if sw.is_failback() {
                                crate::log_debug!(
                                    "[tunnel] relay fail-back: {} -> {} (primary passed its health probes)",
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
                            publish_relay(&selector);
                            super::relay_list::request_refresh("relay switch");
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
                        // 0.46.3: through decide_failure (legacy = exactly
                        // on_failure(); DNS mode may hold, logged).
                        let switch = decide_failure(&mut selector, &mut hints);
                        publish_failover(&failover_slot, &selector);
                        if let Some(sw) = switch {
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
                            publish_relay(&selector);
                            super::relay_list::request_refresh("relay switch");
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
                // disable-unpair pre-mortem). Terminal on block. The same
                // fresh read carries a changed relay list into this
                // reconnect (0.45.7).
                match spawn_gate() {
                    Err(block) => {
                        crate::log_debug!(
                            "[tunnel] respawn blocked — supervisor exiting: {block}"
                        );
                        running.store(false, Ordering::SeqCst);
                        return;
                    }
                    Ok(disk) => {
                        adopt_disk_list(&mut selector, &disk);
                        publish_relay(&selector);
                    }
                }
                watch = new_watch();
                // Re-render on EVERY spawn (2026-10-14): respawning with the
                // same `-c` path against a config that was deleted (or that
                // macOS purged from the per-user temp dir after ~3 days
                // untouched) crash-looped frpc for two hours.
                match render_and_spawn(
                    &frpc_thread,
                    &cfg_thread,
                    selector.current(),
                    frozen_port(),
                    e2e,
                    watch.clone(),
                    &proxy,
                    &health,
                ) {
                    Ok((c, stop)) => {
                        tail_stop = stop;
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

/// Render the config for `relay` (atomic tmp+rename, 0600) and spawn frpc
/// against it. EVERY frpc spawn goes through here (2026-10-14): frpc
/// reads its config only at startup, and the supervisor used to respawn
/// with the same `-c` path without re-rendering, so a config deleted from
/// the private runtime dir (a test daemon did it; macOS also purges files
/// in the per-user temp dir untouched for ~3 days) crash-looped frpc on
/// `open …/frpc.toml: no such file or directory` until a manual stop+start.
/// A render or spawn error counts as a failed run.
#[allow(clippy::too_many_arguments)]
fn render_and_spawn(
    frpc: &Path,
    cfg: &TunnelConfig,
    relay: &RelayEndpoint,
    local_port: u16,
    e2e: bool,
    watch: Option<Arc<SessionWatch>>,
    proxy: &Arc<ProxyRegistration>,
    health: &Arc<FrpcHealth>,
) -> Result<(FrpcProc, Arc<AtomicBool>), String> {
    if let Err(e) = write_relay_config(cfg, relay, local_port, e2e) {
        let msg = format!("render frpc config: {e}");
        health.fail_with(msg.clone());
        return Err(msg);
    }
    spawn_once(frpc, watch, proxy, health).map_err(|e| {
        health.fail_with(e.clone());
        e
    })
}

/// Spawn a single `frpc -c <config>` child that writes stdout+stderr
/// straight into the append-mode `~/.k2/frpc.log` (no pipe: a pipe would
/// kill frpc with `SIGPIPE` once this daemon exits — runtime stage H), in
/// its own session. A tail thread feeds every new line to the mid-session
/// watchdog (multi-relay) and the proxy-registration tracker, so the
/// `~/.k2/frpc.log` contract is untouched. Records `frpc.pid` for the next
/// daemon. Returns the child and its tail's stop flag. Callers go through
/// [`render_and_spawn`].
fn spawn_once(
    frpc: &Path,
    watch: Option<Arc<SessionWatch>>,
    proxy: &Arc<ProxyRegistration>,
    health: &Arc<FrpcHealth>,
) -> Result<(FrpcProc, Arc<AtomicBool>), String> {
    crate::handoff_state::guard()?;
    let cfg_path = frpc_config_path();
    // A new run: not registered until this child logs success.
    let run = proxy.begin_run();
    let log = open_log()?;
    let log_err = log
        .try_clone()
        .map_err(|e| format!("clone log handle: {e}"))?;
    let offset = frpc_proc::log_len(&frpc_log_path());
    let mut cmd = Command::new(frpc);
    cmd.arg("-c")
        .arg(&cfg_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    // Its own session: never in the daemon's process group or session, so
    // a signal to the daemon's group (or a terminal hangup) misses it.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    // frpc is a console-subsystem binary; without this, each tunnel start
    // flashes (or leaves) a black cmd window on the Windows desktop.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let child = cmd
        .spawn()
        .map_err(|e| format!("spawn frpc ({}): {e}", frpc.display()))?;
    let pid = child.id() as i32;
    if let Some(identity) = frpc_proc::proc_identity(pid) {
        frpc_proc::write_pid_record(&frpc_proc::PidRecord {
            pid,
            identity,
            cfg: cfg_path.clone(),
            log_offset: offset,
        });
    }
    let stop = Arc::new(AtomicBool::new(false));
    tail_run(offset, pid, stop.clone(), watch, (proxy.clone(), run), health.clone());
    Ok((FrpcProc::Spawned(child), stop))
}

/// Supervise an frpc the previous daemon left running (stage H). Replays
/// the run's log so far into the proxy-registration tracker (its
/// `start proxy success` line was written before this daemon existed),
/// then tails new lines as for a spawned child. Old lines never reach the
/// watchdog: they carry no time this daemon can trust.
fn adopt_once(
    rec: &frpc_proc::PidRecord,
    watch: Option<Arc<SessionWatch>>,
    proxy: &Arc<ProxyRegistration>,
    health: &Arc<FrpcHealth>,
) -> (FrpcProc, Arc<AtomicBool>) {
    let run = proxy.begin_run();
    let log = frpc_log_path();
    let now = frpc_proc::log_len(&log);
    if let Ok(bytes) = std::fs::read(&log) {
        let from = (rec.log_offset as usize).min(bytes.len());
        let to = (now as usize).min(bytes.len()).max(from);
        for line in String::from_utf8_lossy(&bytes[from..to]).lines() {
            proxy.observe_line(run, line);
        }
    }
    let stop = Arc::new(AtomicBool::new(false));
    tail_run(now, rec.pid, stop.clone(), watch, (proxy.clone(), run), health.clone());
    (FrpcProc::Adopted { pid: rec.pid }, stop)
}

fn tail_run(
    offset: u64,
    pid: i32,
    stop: Arc<AtomicBool>,
    watch: Option<Arc<SessionWatch>>,
    proxy: (Arc<ProxyRegistration>, u64),
    health: Arc<FrpcHealth>,
) {
    frpc_proc::spawn_tail(frpc_log_path(), offset, pid, stop, move |l| {
        handle_line(l, watch.as_deref(), &proxy, &health);
    });
}

/// One frpc log line from the child of `proxy.1`.
fn handle_line(
    line: &str,
    watch: Option<&SessionWatch>,
    proxy: &(Arc<ProxyRegistration>, u64),
    health: &FrpcHealth,
) {
    if let Some(w) = watch {
        w.observe_line(line);
    }
    proxy.0.observe_line(proxy.1, line);
    // A dead run's late error line must not land on a healthy new run.
    if proxy.0.is_current_run(proxy.1) {
        health.observe_line(line);
    }
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
            let empty = crate::test_env::unique_temp_path("frpc-empty");
            std::fs::create_dir_all(&empty).expect("mk empty dir");
            // Per-thread lookup PATH: the process PATH is never changed.
            let res = crate::terminal::path_env::with_lookup_path(&empty, || {
                resolve_frpc(&FrpcBinary::Auto)
            });
            let _ = std::fs::remove_dir_all(&empty);
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
            let bin_dir = crate::test_env::unique_temp_path("frpc-bin");
            std::fs::create_dir_all(&bin_dir).expect("mk bin dir");
            let fake = bin_dir.join("frpc");
            crate::test_env::write_executable(&fake, "#!/bin/sh\nexit 0\n");
            let res = crate::terminal::path_env::with_lookup_path(&bin_dir, || {
                resolve_frpc(&FrpcBinary::Auto)
            });
            let _ = std::fs::remove_dir_all(&bin_dir);
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
            let _e2e = crate::test_env::EnvVar::remove("K2_E2E"); // follow config (default-on)
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
            let _e2e = crate::test_env::EnvVar::remove("K2_E2E");
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

            let _ = stop("test teardown");
        });
    }

    /// PRD connect-login-edge-only I3: with E2E OFF the connector resolves
    /// frpc's `localPort` to the daemon-registered TUNNEL-INGRESS port, not
    /// the cleartext main `default_local_port`; a failing hook is loud.
    #[test]
    fn e2e_off_start_resolves_tunnel_ingress_port_via_hook() {
        with_temp_home(|| {
            config::save(&TunnelConfig {
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                e2e: false,
                ..Default::default()
            })
            .expect("seed config");
            let hook = super::super::ingress::TestIngressHook::install(Box::new(|| Ok(46123)));
            let st = start(None, 57839, &FrpcBinary::Explicit(true_bin()))
                .expect("start with e2e off");
            assert!(st.running);
            let toml = std::fs::read_to_string(frpc_config_path())
                .expect("frpc.toml must have been rendered");
            assert!(
                toml.contains("localPort = 46123"),
                "E2E-off start must target the tunnel-ingress port, not the main port\n{toml}"
            );
            assert!(
                !toml.contains("localPort = 57839"),
                "main loopback port must never be the tunnel target\n{toml}"
            );
            assert!(toml.contains("type = \"http\""), "{toml}");
            let _ = stop("test teardown");

            // A failing hook must abort the start (no fallback to :57839).
            drop(hook);
            let _hook = super::super::ingress::TestIngressHook::install(Box::new(|| {
                Err("tunnel-ingress bind failed".to_string())
            }));
            let err = start(None, 57839, &FrpcBinary::Explicit(true_bin()))
                .expect_err("start must fail when the ingress listener cannot bind");
            assert!(err.contains("tunnel-ingress bind failed"), "{err}");
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
                // 0.46.3: the old-path switch — 0.46.2 rotation, end to end.
                failover: Some("legacy".to_string()),
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

            stop("test").expect("stop");
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
            // 0.46.3: the standby a DNS-mode move goes to must answer.
            let _reach = TestReachability::install(|_| true);
            // 0.46.3 default (DNS) rules: k2.dev says the standby is the
            // live relay, so the 3rd watchdog kill moves there at once.
            super::super::relay_list::reset_state_for_test();
            super::super::relay_list::set_dns_for_test(
                Some(RelayEndpoint { host: "10.0.0.2".to_string(), port: 7000 }),
                Duration::ZERO,
            );
            // Fake frpc: one success line, then an error storm, never exits.
            // The storm rate (10 lines/s) crosses the watchdog threshold
            // (3 in 30s) almost immediately.
            let dir = crate::test_env::unique_temp_path("fake-frpc");
            std::fs::create_dir_all(&dir).expect("mk fake-frpc dir");
            let script = dir.join("frpc-stuck.sh");
            crate::test_env::write_executable(&script, "#!/bin/sh\n\
                 echo '[I] [service.go:299] login to server success, get run id [test]'\n\
                 while true; do\n\
                 \techo '[W] [service.go:132] connect to server error: dial tcp 10.0.0.1:7000: i/o timeout'\n\
                 \tsleep 0.1\n\
                 done\n");

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

            stop("test").expect("stop");
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
            let _ = stop("test teardown");
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
            // 0.46.3: the standby a DNS-mode move goes to must answer.
            let _reach = TestReachability::install(|_| true);
            // DNS rules with no k2.dev answer and a zero minimum outage:
            // the move happens at the plain 3-failure threshold.
            super::super::relay_list::reset_state_for_test();
            let _gate = TestDnsMinOutage::install(Duration::ZERO);
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

            stop("test").expect("stop");
            assert!(!status().running);
        });
    }

    // ── 0.46.3 DNS-aware failover, end to end through the supervisor ──

    fn ten(last: u8) -> RelayEndpoint {
        RelayEndpoint { host: format!("10.0.0.{last}"), port: 7000 }
    }

    fn seed_two_relays() {
        config::save(&TunnelConfig {
            relays: vec![ten(1), ten(2)],
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            e2e: false, // skip the HTTPS-listener path in this unit context
            ..Default::default()
        })
        .expect("seed two-relay config");
    }

    fn failover_state() -> Option<(String, String, String)> {
        status().failover.map(|f| (f.mode, f.state, f.detail))
    }

    fn dialing(host: &str) -> bool {
        std::fs::read_to_string(frpc_config_path())
            .map(|t| t.contains(&format!("serverAddr = \"{host}\"")))
            .unwrap_or(false)
    }

    #[test]
    fn dns_supervisor_stays_on_the_primary_while_k2dev_says_the_standby_is_inactive() {
        with_temp_home(|| {
            // 0.46.3: the standby a DNS-mode move goes to must answer.
            let _reach = TestReachability::install(|_| true);
            super::super::relay_list::reset_state_for_test();
            super::super::relay_list::set_dns_for_test(Some(ten(1)), Duration::ZERO);
            seed_two_relays();
            let st = start(None, 57839, &FrpcBinary::Explicit(true_bin())).expect("start");
            assert!(st.running);
            let f = st.failover.expect("two relays report failover");
            assert_eq!((f.mode.as_str(), f.state.as_str()), ("dns", "ready"), "{f:?}");

            // /bin/true fails every dial fast: past the threshold the
            // supervisor must hold on the primary, not rotate.
            wait_until("the DNS hold", 30, || {
                failover_state().is_some_and(|(_, s, _)| s == "waiting_for_dns")
            });
            let (_, _, detail) = failover_state().expect("status");
            assert!(detail.contains("standby inactive per k2.dev"), "{detail}");
            std::thread::sleep(Duration::from_secs(5));
            assert!(dialing("10.0.0.1"), "must keep dialing the primary");
            assert_eq!(status().server_addr.as_deref(), Some("10.0.0.1"));
            assert_eq!(status().relay_primary, Some(true));

            // The DNS watcher flips the name: the next failure follows it.
            super::super::relay_list::set_dns_for_test(Some(ten(2)), Duration::ZERO);
            wait_until("the move to the now-active standby", 60, || dialing("10.0.0.2"));
            wait_until("status to report the standby", 10, || {
                status().server_addr.as_deref() == Some("10.0.0.2")
            });
            assert_eq!(status().relay_primary, Some(false));
            assert_eq!(failover_state().expect("status").1, "on_fallback");
            stop("test").expect("stop");
            super::super::relay_list::reset_state_for_test();
        });
    }

    #[test]
    fn dns_supervisor_with_k2dev_silent_waits_the_minimum_outage_then_moves() {
        with_temp_home(|| {
            // 0.46.3: the standby a DNS-mode move goes to must answer.
            let _reach = TestReachability::install(|_| true);
            super::super::relay_list::reset_state_for_test(); // no answer: Unknown
            let _gate = TestDnsMinOutage::install(Duration::from_secs(6));
            seed_two_relays();
            let started = Instant::now();
            start(None, 57839, &FrpcBinary::Explicit(true_bin())).expect("start");
            wait_until("the DNS hold", 30, || {
                failover_state().is_some_and(|(_, s, _)| s == "waiting_for_dns")
            });
            let (_, _, detail) = failover_state().expect("status");
            assert!(detail.contains("did not say"), "{detail}");
            assert!(dialing("10.0.0.1"), "held on the primary first");
            wait_until("the move after the minimum outage", 60, || dialing("10.0.0.2"));
            assert!(
                started.elapsed() >= Duration::from_secs(6),
                "moved before the minimum outage: {:?}",
                started.elapsed()
            );
            stop("test").expect("stop");
        });
    }

    #[test]
    fn dns_supervisor_with_k2dev_silent_never_moves_inside_150s() {
        // The production minimum (no override): a fast-failing primary for
        // ~10 s never rotates when k2.dev gives no answer.
        with_temp_home(|| {
            super::super::relay_list::reset_state_for_test();
            seed_two_relays();
            start(None, 57839, &FrpcBinary::Explicit(true_bin())).expect("start");
            wait_until("the DNS hold", 30, || {
                failover_state().is_some_and(|(_, s, _)| s == "waiting_for_dns")
            });
            std::thread::sleep(Duration::from_secs(8));
            assert!(dialing("10.0.0.1"), "must not leave the primary inside 150 s");
            assert_eq!(status().relay_primary, Some(true));
            stop("test").expect("stop");
        });
    }

    #[test]
    fn dns_supervisor_fails_back_fast_once_k2dev_points_the_name_at_the_primary() {
        with_temp_home(|| {
            // 0.46.3: the standby a DNS-mode move goes to must answer.
            let _reach = TestReachability::install(|_| true);
            super::super::relay_list::reset_state_for_test();
            // Fast DNS-mode timing: probe every 1 s, 2 healthy probes.
            let _policy = TestFailbackPolicy::install(super::super::failover::FailbackPolicy {
                probe_every: Duration::from_secs(1),
                probe_spread: Duration::ZERO,
                healthy_probes: 2,
                failback_delay_max: Duration::ZERO,
            });
            let primary = std::net::TcpListener::bind("127.0.0.1:0").expect("bind primary");
            let p_port = primary.local_addr().expect("addr").port();
            let fallback = crate::test_env::ClosedPort::new();
            let f_port = fallback.port();
            let p_ep = RelayEndpoint { host: "127.0.0.1".to_string(), port: p_port };
            let f_ep = RelayEndpoint { host: "127.0.0.1".to_string(), port: f_port };
            // The name is on the standby (DNS flipped during the outage).
            super::super::relay_list::set_dns_for_test(Some(f_ep.clone()), Duration::ZERO);

            let dir = crate::test_env::unique_temp_path("fake-frpc-dns-failback");
            std::fs::create_dir_all(&dir).expect("mk dir");
            let script = dir.join("frpc-pick.sh");
            crate::test_env::write_executable(
                &script,
                &format!(
                    "#!/bin/sh\nport=$(grep '^serverPort' \"$2\" | tr -dc 0-9)\n\
                     if [ \"$port\" = \"{p_port}\" ] && [ ! -e '{allow}' ]; then exit 1; fi\n\
                     exec sleep 600\n",
                    allow = dir.join("allow").display(),
                ),
            );
            config::save(&TunnelConfig {
                relays: vec![p_ep.clone(), f_ep.clone()],
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                e2e: false,
                ..Default::default()
            })
            .expect("seed two-relay config");
            start(None, 57839, &FrpcBinary::Explicit(script)).expect("start");
            wait_until("failover to the active standby", 30, || {
                status().server_port == Some(f_port)
            });
            // The primary answers TCP again, but k2.dev still says the
            // name is on the standby: going back would take the box off
            // its public name, so it stays.
            std::fs::write(dir.join("allow"), b"").expect("allow primary");
            wait_until("the name-here hold", 15, || {
                failover_state().is_some_and(|(_, _, d)| d.contains("points here"))
            });
            std::thread::sleep(Duration::from_secs(4));
            assert_eq!(status().server_port, Some(f_port), "no fail-back while the name is here");

            // k2.dev: the name is back on the primary → 2 healthy probes.
            super::super::relay_list::set_dns_for_test(Some(p_ep), Duration::ZERO);
            wait_until("fail-back to the primary", 30, || status().server_port == Some(p_port));
            assert_eq!(status().relay_primary, Some(true));
            assert_eq!(failover_state().expect("status").1, "ready");
            stop("test").expect("stop");
            drop(primary);
            let _ = std::fs::remove_dir_all(&dir);
            super::super::relay_list::reset_state_for_test();
        });
    }

    #[test]
    fn dns_supervisor_with_no_relay_reachable_stays_on_the_primary_and_backs_off() {
        // z3mbpZ on a dead network: 0.46.2 alternated every ~6 s. Even
        // with every switch rule met (k2.dev names the standby, zero
        // minimum outage) an unreachable standby is never switched to.
        with_temp_home(|| {
            super::super::relay_list::reset_state_for_test();
            super::super::relay_list::set_dns_for_test(Some(ten(2)), Duration::ZERO);
            let _gate = TestDnsMinOutage::install(Duration::ZERO);
            let _reach = TestReachability::install(|_| false);
            let dir = crate::test_env::unique_temp_path("fake-frpc-offline");
            std::fs::create_dir_all(&dir).expect("mk dir");
            let script = dir.join("frpc-offline.sh");
            // Every dial fails; each one is recorded (host per line).
            crate::test_env::write_executable(
                &script,
                &format!(
                    "#!/bin/sh\ngrep '^serverAddr' \"$2\" | tr -d '\" ' | cut -d= -f2 >> '{dials}'\nexit 1\n",
                    dials = dir.join("dials").display(),
                ),
            );
            seed_two_relays();
            start(None, 57839, &FrpcBinary::Explicit(script)).expect("start");
            wait_until("the no-relay hold", 30, || {
                failover_state().is_some_and(|(_, s, _)| s == "no_relay_reachable")
            });
            std::thread::sleep(Duration::from_secs(12));
            let dials = std::fs::read_to_string(dir.join("dials")).expect("dials");
            let hosts: Vec<&str> = dials.lines().collect();
            assert!(
                hosts.iter().all(|h| *h == "10.0.0.1"),
                "must only ever dial the primary: {hosts:?}"
            );
            // Backoff 0.5, 1, 2, 4, 8 s… (never reset by a switch): a
            // handful of dials, not one every second or two.
            assert!(hosts.len() <= 8, "{} dials — the backoff must grow: {hosts:?}", hosts.len());
            assert_eq!(status().relay_primary, Some(true));
            stop("test").expect("stop");
            let _ = std::fs::remove_dir_all(&dir);
            super::super::relay_list::reset_state_for_test();
        });
    }


    #[test]
    fn legacy_switch_reports_legacy_and_never_asks_k2dev() {
        with_temp_home(|| {
            super::super::relay_list::reset_state_for_test();
            // Even a k2.dev answer naming the primary is ignored in legacy.
            super::super::relay_list::set_dns_for_test(Some(ten(1)), Duration::ZERO);
            let _env = crate::test_env::EnvVar::set(config::FAILOVER_ENV, "legacy");
            seed_two_relays();
            let st = start(None, 57839, &FrpcBinary::Explicit(true_bin())).expect("start");
            let f = st.failover.expect("two relays report failover");
            assert_eq!((f.mode.as_str(), f.state.as_str()), ("legacy", "legacy"));
            wait_until("the 0.46.2 rotation after 3 failures", 30, || dialing("10.0.0.2"));
            stop("test").expect("stop");
            super::super::relay_list::reset_state_for_test();
        });
    }

    #[test]
    fn solo_relay_reports_no_failover() {
        with_temp_home(|| {
            config::save(&TunnelConfig {
                server_addr: "9.9.9.9".to_string(),
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                e2e: false,
                ..Default::default()
            })
            .expect("seed");
            let st = start(None, 57839, &FrpcBinary::Explicit(true_bin())).expect("start");
            assert_eq!(st.failover, None);
            let _ = stop("test");
        });
    }


    /// A fake frpc that stays up (a healthy, connected tunnel) and records
    /// each launch in `<dir>/spawns`.
    fn long_lived_frpc(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).expect("mk fake-frpc dir");
        let script = dir.join("frpc-up.sh");
        crate::test_env::write_executable(
            &script,
            &format!(
                "#!/bin/sh\necho spawn >> '{}'\nexec sleep 600\n",
                dir.join("spawns").display()
            ),
        );
        script
    }

    fn spawn_count(dir: &Path) -> usize {
        std::fs::read_to_string(dir.join("spawns"))
            .expect("the fake frpc must have recorded its launches")
            .lines()
            .count()
    }

    /// Setting `relays` through the UPDATE path (`set_config`) must not
    /// disturb a currently-connected tunnel — exactly like server_addr,
    /// it's persist-only. Guards the "no live restart" contract end to end:
    /// a healthy running tunnel keeps its child, endpoint and rendered
    /// frpc.toml; a stop/start dials the new primary. (0.45.7: a RECONNECT
    /// also picks the list up — see the next test.)
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

            let dir = crate::test_env::unique_temp_path("fake-frpc-up");
            let frpc = long_lived_frpc(&dir);
            let st = start(None, 57839, &FrpcBinary::Explicit(frpc.clone()))
                .expect("start on legacy endpoint");
            assert!(st.running);
            wait_until("the first frpc launch", 10, || dir.join("spawns").exists());

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
            // Give a (wrong) live restart time to happen, then prove none did.
            std::thread::sleep(Duration::from_secs(3));
            assert_eq!(spawn_count(&dir), 1, "a list change alone must not restart frpc");
            assert_eq!(status().server_addr.as_deref(), Some("9.9.9.9"));

            // The NEXT start picks the new list up: dials the new primary.
            stop("test").expect("stop");
            let st = start(None, 57839, &FrpcBinary::Explicit(frpc))
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
            stop("test").expect("stop");
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// 0.45.7: a relay list written while the tunnel runs (k2.dev refresh,
    /// pairing) is applied at the next RECONNECT — here /bin/true exits at
    /// once, so the supervisor reconnects continually and must dial the new
    /// primary without anyone calling stop/start.
    #[test]
    fn a_changed_relay_list_is_applied_at_the_next_reconnect() {
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
            start(None, 57839, &FrpcBinary::Explicit(true_bin())).expect("start");
            config::update(|c| {
                c.relays = vec![
                    RelayEndpoint { host: "10.0.0.1".to_string(), port: 7000 },
                    RelayEndpoint { host: "10.0.0.2".to_string(), port: 7000 },
                ];
            })
            .expect("write a new list");
            wait_until("the reconnect to adopt the new list", 30, || {
                std::fs::read_to_string(frpc_config_path())
                    .map(|t| t.contains("serverAddr = \"10.0.0."))
                    .unwrap_or(false)
            });
            let st = status();
            assert!(st.running);
            assert!(
                st.server_addr.as_deref().is_some_and(|a| a.starts_with("10.0.0.")),
                "status must follow the adopted list: {:?}",
                st.server_addr
            );
            assert_eq!(st.relay_list.relays, vec!["10.0.0.1:7000", "10.0.0.2:7000"]);
            stop("test").expect("stop");
        });
    }

    /// 0.45.7, the 2026-10-15 shape: a SOLO tunnel whose only relay died
    /// mid-session (frpc alive, storming reconnect errors) stays put while
    /// the list is unchanged — frpc's own retry, as before — and reconnects
    /// to the new list once `tunnel.json` gains relays.
    #[test]
    fn solo_dead_session_reconnects_only_when_the_list_gains_relays() {
        with_temp_home(|| {
            let dir = crate::test_env::unique_temp_path("fake-frpc-solo");
            std::fs::create_dir_all(&dir).expect("mk dir");
            let script = dir.join("frpc-stuck.sh");
            crate::test_env::write_executable(
                &script,
                &format!(
                    "#!/bin/sh\necho spawn >> '{}'\n\
                     echo '[I] [service.go:299] login to server success, get run id [test]'\n\
                     while true; do\n\
                     \techo '[W] [service.go:132] connect to server error: dial tcp 9.9.9.9:7009: i/o timeout'\n\
                     \tsleep 0.1\n\
                     done\n",
                    dir.join("spawns").display()
                ),
            );
            config::save(&TunnelConfig {
                server_addr: "9.9.9.9".to_string(),
                server_port: 7009,
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                e2e: false,
                ..Default::default()
            })
            .expect("seed legacy config");
            // Launches recorded so far (0 before the fake has written its
            // line — a polling condition, never an assertion default).
            let launches = || {
                std::fs::read_to_string(dir.join("spawns"))
                    .map(|s| s.lines().count())
                    .unwrap_or(0)
            };
            let checks_before = SOLO_LIST_UNCHANGED_CHECKS.load(Ordering::SeqCst);
            start(None, 57839, &FrpcBinary::Explicit(script)).expect("start");
            wait_until("the first fake frpc launch", 10, || launches() >= 1);
            // Wait on the DECISION, not a clock: the watchdog declares the
            // session dead, and the solo supervisor re-reads the (unchanged)
            // list three times and leaves frpc's own retry alone each time.
            // (A stale supervisor from an earlier test can add at most one.)
            wait_until("three unchanged-list checks on the dead solo session", 30, || {
                SOLO_LIST_UNCHANGED_CHECKS.load(Ordering::SeqCst) >= checks_before + 4
            });
            assert_eq!(spawn_count(&dir), 1, "unchanged list → leave frpc's own retry alone");
            assert_eq!(status().server_addr.as_deref(), Some("9.9.9.9"));

            config::update(|c| {
                c.relays = vec![
                    RelayEndpoint { host: "10.0.0.1".to_string(), port: 7000 },
                    RelayEndpoint { host: "10.0.0.2".to_string(), port: 7000 },
                ];
            })
            .expect("the refreshed list lands");
            // The supervisor renders frpc.toml for the new relay BEFORE it
            // launches the new child, so wait for both (2026-10-15 flake:
            // the launch count was read between the two).
            wait_until("the dead solo session to reconnect on the new list", 30, || {
                launches() >= 2
                    && std::fs::read_to_string(frpc_config_path())
                        .map(|t| t.contains("serverAddr = \"10.0.0.1\""))
                        .unwrap_or(false)
            });
            stop("test").expect("stop");
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// Fast fail-back timing for the supervisor tests: probe every 1 s, 3
    /// healthy probes, no random delay.
    fn fast_failback() -> TestFailbackPolicy {
        TestFailbackPolicy::install(super::super::failover::FailbackPolicy {
            probe_every: Duration::from_secs(1),
            probe_spread: Duration::ZERO,
            healthy_probes: 3,
            failback_delay_max: Duration::ZERO,
        })
    }

    /// 0.45.7 fail-back, end to end: the primary refuses frpc at first (the
    /// fake exits at once for its port), the supervisor fails over, then —
    /// with the primary's port answering TCP — 3 healthy probes bring it
    /// back to the primary with one reconnect.
    #[test]
    fn supervisor_fails_back_to_the_primary_after_healthy_probes() {
        with_temp_home(|| {
            let _policy = fast_failback();
            // The primary "relay": a bound loopback port (TCP connects
            // succeed from the backlog; nothing needs to accept).
            let primary = std::net::TcpListener::bind("127.0.0.1:0").expect("bind primary");
            let p_port = primary.local_addr().expect("addr").port();
            let fallback = crate::test_env::ClosedPort::new();
            let f_port = fallback.port();

            let dir = crate::test_env::unique_temp_path("fake-frpc-failback");
            std::fs::create_dir_all(&dir).expect("mk dir");
            let script = dir.join("frpc-pick.sh");
            // Dials to the primary fail fast until `allow` exists; dials
            // anywhere else stay up. Every dial is recorded.
            crate::test_env::write_executable(
                &script,
                &format!(
                    "#!/bin/sh\nport=$(grep '^serverPort' \"$2\" | tr -dc 0-9)\n\
                     echo $port >> '{dials}'\n\
                     if [ \"$port\" = \"{p_port}\" ] && [ ! -e '{allow}' ]; then exit 1; fi\n\
                     exec sleep 600\n",
                    dials = dir.join("dials").display(),
                    allow = dir.join("allow").display(),
                ),
            );
            config::save(&TunnelConfig {
                relays: vec![
                    RelayEndpoint { host: "127.0.0.1".to_string(), port: p_port },
                    RelayEndpoint { host: "127.0.0.1".to_string(), port: f_port },
                ],
                // The 0.45.7/0.46.2 probe-driven fail-back, end to end.
                failover: Some("legacy".to_string()),
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                e2e: false,
                ..Default::default()
            })
            .expect("seed two-relay config");
            start(None, 57839, &FrpcBinary::Explicit(script)).expect("start");

            wait_until("failover to the fallback", 30, || {
                status().server_port == Some(f_port)
            });
            let st = status();
            assert_eq!(st.relay_primary, Some(false), "on the fallback");
            // The primary now takes frpc again; its port answers probes.
            std::fs::write(dir.join("allow"), b"").expect("allow primary");
            wait_until("fail-back to the primary", 30, || {
                status().server_port == Some(p_port)
            });
            assert_eq!(status().relay_primary, Some(true));
            wait_until("the primary to be dialed again", 10, || {
                std::fs::read_to_string(dir.join("dials"))
                    .map(|d| d.lines().last() == Some(p_port.to_string().as_str()))
                    .unwrap_or(false)
            });
            stop("test").expect("stop");
            drop(primary);
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// 0.45.7: while the primary is the relay in use nothing probes it and
    /// nothing fails back.
    #[test]
    fn no_failback_probes_while_on_the_primary() {
        with_temp_home(|| {
            let _policy = fast_failback();
            let primary = std::net::TcpListener::bind("127.0.0.1:0").expect("bind primary");
            primary.set_nonblocking(true).expect("nonblocking");
            let p_port = primary.local_addr().expect("addr").port();
            let dir = crate::test_env::unique_temp_path("fake-frpc-onprimary");
            let frpc = long_lived_frpc(&dir);
            config::save(&TunnelConfig {
                relays: vec![
                    RelayEndpoint { host: "127.0.0.1".to_string(), port: p_port },
                    RelayEndpoint { host: "127.0.0.1".to_string(), port: 9 },
                ],
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                e2e: false,
                ..Default::default()
            })
            .expect("seed");
            start(None, 57839, &FrpcBinary::Explicit(frpc)).expect("start");
            std::thread::sleep(Duration::from_secs(4));
            match primary.accept() {
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Ok((_, from)) => panic!("the primary in use was probed (connection from {from})"),
                Err(e) => panic!("accept: {e}"),
            }
            let st = status();
            assert_eq!(st.relay_primary, Some(true));
            assert_eq!(st.server_port, Some(p_port));
            assert_eq!(spawn_count(&dir), 1, "no reconnect while on the primary");
            stop("test").expect("stop");
            let _ = std::fs::remove_dir_all(&dir);
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
            stop("test").expect("stop");
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
                ..Default::default()
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
            stop("test").expect("stop");
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
            stop("test").expect("stop is idempotent after the gate exit");
        });
    }

    #[test]
    fn proxy_registration_tracks_only_the_current_run() {
        let reg = ProxyRegistration::new(&TunnelConfig {
            subdomain: "rosson".to_string(),
            ..Default::default()
        });
        assert!(!reg.is_registered(), "never spawned");
        let r1 = reg.begin_run();
        let logins = frpc_login_count();
        reg.observe_line(r1, "[I] [service.go:299] login to server success, get run id [x]");
        assert!(!reg.is_registered(), "login alone is not registration");
        assert!(frpc_login_count() > logins, "a login moves the login count (Sew TURN DNS retry)");
        reg.observe_line(r1, "[I] [proxy_wrapper.go:204] [x] [k2so-other] start proxy success");
        assert!(!reg.is_registered(), "another proxy's success does not count");
        reg.observe_line(r1, "[I] [proxy_wrapper.go:204] [x] [k2so-rosson2] start proxy success");
        assert!(!reg.is_registered(), "a longer name sharing the prefix does not count");
        reg.observe_line(r1, "[I] [proxy_wrapper.go:204] [x] [k2so-rosson] start proxy success");
        assert!(reg.is_registered());
        // A later refusal for our proxy (re-login, name taken) clears it.
        reg.observe_line(r1, "[W] [proxy_wrapper.go:180] [x] [k2so-rosson] start error: proxy [k2so-rosson] already exists");
        assert!(!reg.is_registered());
        reg.observe_line(r1, "[I] [proxy_wrapper.go:204] [x] [k2so-rosson] start proxy success");
        assert!(reg.is_registered());
        // A respawn clears it; the previous child's late output is ignored.
        let r2 = reg.begin_run();
        assert!(!reg.is_registered(), "a new run starts unregistered");
        reg.observe_line(r1, "[I] [proxy_wrapper.go:204] [x] [k2so-rosson] start proxy success");
        assert!(!reg.is_registered(), "a stale run's success must not count");
        reg.observe_line(r2, "[I] [proxy_wrapper.go:204] [x] [k2so-rosson] start proxy success");
        assert!(reg.is_registered());
    }

    #[test]
    fn a_dead_runs_late_error_line_does_not_mark_a_healthy_run() {
        let reg = Arc::new(ProxyRegistration::new(&TunnelConfig {
            subdomain: "rosson".to_string(),
            ..Default::default()
        }));
        let health = FrpcHealth::new();
        let r1 = reg.begin_run();
        let r2 = reg.begin_run();
        health.record_up();
        assert_eq!(health.last_error(), None);
        let err = "[E] [sub/root.go:142] open /tmp/x/.k2/frpc.toml: no such file or directory";
        handle_line(err, None, &(reg.clone(), r1), &health);
        assert_eq!(health.last_error(), None, "a dead run's late error must be ignored");
        handle_line(err, None, &(reg.clone(), r2), &health);
        assert_eq!(health.last_error().as_deref(), Some(err));
    }

    #[test]
    fn the_registration_clears_when_the_frps_session_is_lost() {
        // Real frp 0.61 lines (from a K2 box's frpc.log, ANSI colour kept).
        const OK: &str = "\u{1b}[0m\u{1b}[1;34m2026-06-03 16:20:11.804 [I] [client/control.go:165] [a7c3215c5a5056eb] [k2so-rosson] start proxy success";
        let lost = [
            "\u{1b}[0m\u{1b}[1;33m2026-06-03 22:06:31.021 [W] [client/service.go:319] [c8e85694758a289f] connect to server error: dial tcp 178.156.232.105:7000: connect: network is unreachable",
            "\u{1b}[0m\u{1b}[1;34m2026-06-03 17:19:51.768 [I] [client/service.go:308] [b3b887f43fa3967e] try to connect to server...",
            "\u{1b}[0m\u{1b}[1;33m2026-06-03 18:32:27.570 [W] [client/control.go:119] [b3b887f43fa3967e] start new connection to server error: session shutdown",
            "[W] [client/control.go:80] [b3b887f43fa3967e] work connection write to server error: session shutdown",
            "[W] [client/service.go:200] [x] login to server failed: i/o timeout",
            "[I] [client/service.go:150] [x] try to reconnect to server...",
            "[W] [client/control.go:90] [x] work connection closed before response StartWorkConn message: EOF",
        ];
        for line in lost {
            let reg = ProxyRegistration::new(&TunnelConfig {
                subdomain: "rosson".to_string(),
                ..Default::default()
            });
            let run = reg.begin_run();
            reg.observe_line(run, OK);
            assert!(reg.is_registered(), "the success line registers");
            reg.observe_line(run, line);
            assert!(!reg.is_registered(), "a lost session must clear the flag: {line}");
            // Re-login and a new success line register again.
            reg.observe_line(run, "[I] [client/service.go:287] [d1] login to server success, get run id [d1]");
            assert!(!reg.is_registered(), "logging in again is not registration");
            reg.observe_line(run, OK);
            assert!(reg.is_registered());
        }
    }

    #[test]
    fn a_respawn_clears_the_proxy_registration() {
        with_temp_home(|| {
            // Run 1 registers then exits; every later run is refused.
            let dir = crate::test_env::unique_temp_path("fake-frpc");
            std::fs::create_dir_all(&dir).expect("mk fake-frpc dir");
            let counter = dir.join("runs");
            let script = dir.join("frpc");
            crate::test_env::write_executable(
                &script,
                &format!(
                    "#!/bin/sh\n\
                     n=$(cat '{c}' 2>/dev/null || echo 0); n=$((n+1)); echo $n > '{c}'\n\
                     if [ \"$n\" = 1 ]; then\n\
                     \techo '[I] [proxy_wrapper.go:204] [x] [k2so-rosson] start proxy success'\n\
                     \tsleep 1\n\
                     \texit 1\n\
                     fi\n\
                     echo '[W] [proxy_wrapper.go:180] [x] [k2so-rosson] start error: proxy [k2so-rosson] already exists'\n\
                     exec sleep 60\n",
                    c = counter.display()
                ),
            );
            config::save(&TunnelConfig {
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                e2e: false,
                ..Default::default()
            })
            .expect("seed config");
            let _hook = super::super::ingress::TestIngressHook::install(Box::new(|| Ok(46124)));
            let _lease = crate::test_env::EnvVar::set(super::super::lease::LEASE_ENV_VAR, "off");
            start(None, 57839, &FrpcBinary::Explicit(script.clone())).expect("start");

            let deadline = Instant::now() + Duration::from_secs(10);
            while !proxy_registered() {
                assert!(Instant::now() < deadline, "run 1's success line was never seen");
                std::thread::sleep(Duration::from_millis(10));
            }
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let runs = std::fs::read_to_string(&counter).unwrap_or_default();
                let refused = std::fs::read_to_string(frpc_log_path())
                    .unwrap_or_default()
                    .contains("already exists");
                if runs.trim() == "2" && refused {
                    break;
                }
                assert!(Instant::now() < deadline, "the supervisor never respawned (runs={runs:?})");
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(status().running, "the respawned frpc runs");
            assert!(!proxy_registered(), "a respawn must clear the registration");
            assert!(registered_config().is_none());

            stop("test").expect("stop");
            assert!(!proxy_registered());
            // The hook is process-global: leaving it installed made
            // `supervisor_rotation_keeps_frozen_local_port` (which expects
            // the default port) order-dependent.
            super::super::ingress::clear_ensure_tunnel_ingress();
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    fn wait_until(what: &str, secs: u64, mut ok: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while !ok() {
            assert!(Instant::now() < deadline, "timed out waiting for: {what}\nstatus: {:?}", status());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// 2026-10-14 incident, end to end through the REAL supervisor: the
    /// rendered config is deleted under a running tunnel. The fake frpc
    /// behaves like the real one — it refuses to start without its config
    /// (`open …: no such file or directory`) and a live one exits when the
    /// config is gone (standing in for its next restart). The supervisor
    /// must re-render before every respawn, status must say UNHEALTHY while
    /// runs fail (not a bare "running"), and `start()` on a running-but-
    /// broken tunnel must repair it instead of reporting success.
    #[test]
    fn supervisor_rerenders_a_deleted_config_and_status_reports_a_crash_loop() {
        with_temp_home(|| {
            let dir = crate::test_env::unique_temp_path("fake-frpc");
            std::fs::create_dir_all(&dir).expect("mk fake-frpc dir");
            let starts = dir.join("starts");
            let broken = dir.join("broken");
            let hold = dir.join("hold");
            let script = dir.join("frpc");
            crate::test_env::write_executable(
                &script,
                &format!(
                    "#!/bin/sh\n\
                     cfg=\"$2\"\n\
                     if [ -f '{b}' ] || [ ! -f \"$cfg\" ]; then\n\
                     \techo \"[E] [sub/root.go:142] open $cfg: no such file or directory\" >&2\n\
                     \texit 1\n\
                     fi\n\
                     grep -q 'token = \"tok\"' \"$cfg\" || {{ echo '[E] bad config' >&2; exit 1; }}\n\
                     echo started >> '{s}'\n\
                     echo '[I] [proxy_manager.go:173] [x] [k2so-rosson] start proxy success'\n\
                     while [ -f \"$cfg\" ] || [ -f '{h}' ]; do sleep 0.1; done\n\
                     echo '[E] config vanished' >&2\n\
                     exit 1\n",
                    b = broken.display(),
                    s = starts.display(),
                    h = hold.display(),
                ),
            );
            let start_count = || {
                std::fs::read_to_string(&starts).unwrap_or_default().lines().count()
            };
            config::save(&TunnelConfig {
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                e2e: false,
                ..Default::default()
            })
            .expect("seed config");
            // No ingress hook: a registered one outlives the test and
            // changes other tests' resolved port.
            super::super::ingress::clear_ensure_tunnel_ingress();
            let _lease = crate::test_env::EnvVar::set(super::super::lease::LEASE_ENV_VAR, "off");
            let bin = FrpcBinary::Explicit(script.clone());
            start(None, 57839, &bin).expect("start");

            wait_until("first clean frpc start", 10, || start_count() == 1 && proxy_registered());
            let st = status();
            assert_eq!(st.frpc_healthy, Some(true), "{st:?}");
            assert_eq!(st.frpc_consecutive_failures, Some(0), "{st:?}");
            assert_eq!(st.frpc_config_present, Some(true), "{st:?}");

            // 1) Delete the config under the running tunnel: the fake exits,
            //    and the respawn must find a freshly rendered config.
            std::fs::remove_file(frpc_config_path()).expect("delete frpc.toml");
            wait_until("a clean respawn after the config was deleted", 15, || {
                start_count() == 2 && proxy_registered()
            });
            let toml = std::fs::read_to_string(frpc_config_path()).expect("re-rendered frpc.toml");
            assert!(toml.contains("token = \"tok\""), "{toml}");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(frpc_config_path()).expect("stat").permissions().mode();
                assert_eq!(mode & 0o777, 0o600, "re-rendered config must stay 0600");
            }
            assert_eq!(status().frpc_healthy, Some(true));

            // 2) A crash loop: every run fails. Status must say so, with
            //    frpc's own error line, while `running` stays the desired
            //    state.
            std::fs::write(&broken, "1").expect("break the fake");
            std::fs::remove_file(frpc_config_path()).expect("delete frpc.toml again");
            wait_until("status to report the crash loop", 15, || {
                status().frpc_consecutive_failures.unwrap_or(0) >= 2
            });
            let st = status();
            assert!(st.running, "{st:?}");
            assert_eq!(st.frpc_healthy, Some(false), "{st:?}");
            let err = st.frpc_last_error.clone().expect("a last error while failing");
            assert!(err.contains("no such file or directory"), "{err}");
            assert_eq!(st.frpc_config_present, Some(true), "every respawn re-renders: {st:?}");

            // 3) `start()` on a running-but-failing tunnel repairs it.
            let st = start(None, 57839, &bin).expect("repair start");
            let why = st.repaired.clone().expect("start must repair, not report success");
            assert!(why.contains("consecutive failures"), "{why}");
            std::fs::remove_file(&broken).expect("unbreak the fake");
            wait_until("recovery after the repair", 20, || {
                start_count() == 3 && status().frpc_healthy == Some(true)
            });
            assert_eq!(status().frpc_last_error, None);

            // 4) Config gone while frpc keeps running AND frps has its proxy
            //    registered: status says so, and `start()` re-renders the
            //    config in place WITHOUT restarting the live frpc (0.45.7
            //    pause-safety: frpc read the file at spawn; the supervisor
            //    re-renders before any respawn).
            std::fs::write(&hold, "1").expect("hold the fake up");
            wait_until("the held run is registered", 10, || proxy_registered());
            std::fs::remove_file(frpc_config_path()).expect("delete frpc.toml a third time");
            let st = status();
            assert_eq!(st.frpc_healthy, Some(false), "{st:?}");
            assert_eq!(st.frpc_config_present, Some(false), "{st:?}");
            let gen = spawn_generation();
            let st = start(None, 57839, &bin).expect("start over a registered frpc");
            assert_eq!(st.repaired, None, "a registered frpc must not be restarted: {st:?}");
            assert_eq!(spawn_generation(), gen, "no new supervisor");
            assert!(frpc_config_path().exists(), "start re-rendered the config in place");
            assert_eq!(st.frpc_healthy, Some(true), "{st:?}");
            assert_eq!(start_count(), 3, "the live frpc kept running");
            std::fs::remove_file(&hold).expect("release the hold");

            stop("test").expect("stop");
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn status_is_stopped_before_any_start() {
        with_temp_home(|| {
            // Ensure a clean singleton for this test.
            let _ = stop("test teardown");
            assert_eq!(status(), TunnelStatus::stopped());
        });
    }

    #[test]
    fn stop_is_idempotent_on_stopped_connector() {
        with_temp_home(|| {
            stop("test").expect("first stop ok");
            stop("test").expect("second stop ok");
            assert!(!status().running);
        });
    }

    #[test]
    fn stop_records_and_logs_its_reason() {
        with_temp_home(|| {
            stop("POST /cli/tunnel/stop by owner-token via loopback").expect("stop");
            assert_eq!(
                last_stop_reason().as_deref(),
                Some("POST /cli/tunnel/stop by owner-token via loopback")
            );
        });
        assert_eq!(
            stop_log_line("daemon shutdown", Some("grokbot"), true),
            "[tunnel] stop (daemon shutdown): stopping the tunnel for \"grokbot\" \
             (frps proxy registered: true)"
        );
        assert_eq!(
            stop_log_line("update apply", None, false),
            "[tunnel] stop (update apply): no tunnel running; reaping any stray frpc"
        );
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
            let dir = crate::test_env::unique_temp_path("orphan-frpc");
            std::fs::create_dir_all(&dir).expect("mk orphan-frpc dir");
            let frpc = dir.join("frpc");
            crate::test_env::write_executable(&frpc, "#!/bin/sh\nsleep 300\n");

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
            stop("test").expect("stop must succeed with empty STATE");
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
            let dir = crate::test_env::unique_temp_path("solo-stop");
            std::fs::create_dir_all(&dir).expect("mk solo-stop dir");
            // Named `frpc` so stop()'s pattern reap is a second line of defense.
            let script = dir.join("frpc");
            crate::test_env::write_executable(&script, "#!/bin/sh\nsleep 300\n");

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

            stop("test").expect("stop solo tunnel");
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

    /// A long-lived frpc stand-in that registers its proxy, never `exec`s
    /// (its argv stays `<sh> <…/frpc> -c <cfg>`), and logs straight into
    /// the file the connector hands it.
    fn registering_frpc(dir: &Path) -> PathBuf {
        let script = dir.join("frpc");
        crate::test_env::write_executable(
            &script,
            "#!/bin/sh\n\
             echo '[I] [proxy_wrapper.go:204] [x] [k2so-rosson] start proxy success'\n\
             while :; do sleep 1; done\n",
        );
        script
    }

    /// Simulate the daemon process going away after a hand-off: the
    /// supervisor lets go (no kill), and the connector singleton is
    /// forgotten as a fresh daemon would have it.
    fn hand_off_and_forget() -> i32 {
        let pid = detach_for_handoff().expect("a live frpc to hand off");
        // The supervisor polls every second; give it time to let go.
        std::thread::sleep(SUPERVISE_POLL * 2);
        *state().lock().unwrap_or_else(|p| p.into_inner()) = None;
        DETACHED.store(false, Ordering::SeqCst);
        pid
    }

    /// Runtime stage H (AR3/K37): a new daemon adopts the frpc the previous
    /// one left running — same pid, no reap, registration recovered from
    /// the log — and a later stop() still kills it.
    #[test]
    fn a_restarted_daemon_adopts_the_running_frpc() {
        with_temp_home(|| {
            let dir = crate::test_env::unique_temp_path("adopt-frpc");
            std::fs::create_dir_all(&dir).expect("mk dir");
            let script = registering_frpc(&dir);
            config::save(&TunnelConfig {
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                e2e: false,
                ..Default::default()
            })
            .expect("seed config");
            super::super::ingress::register_ensure_tunnel_ingress(Box::new(|| Ok(46124)));
            let _lease = crate::test_env::EnvVar::set(super::super::lease::LEASE_ENV_VAR, "off");

            start(None, 57839, &FrpcBinary::Explicit(script.clone())).expect("start");
            let deadline = Instant::now() + Duration::from_secs(10);
            while !proxy_registered() {
                assert!(Instant::now() < deadline, "registration line never seen");
                std::thread::sleep(Duration::from_millis(20));
            }
            let rec = frpc_proc::read_pid_record(&frpc_config_path()).expect("pid record");

            let pid = hand_off_and_forget();
            assert_eq!(pid, rec.pid);
            assert!(frpc_proc::pid_alive(pid), "hand-off must leave frpc running");
            assert!(!status().running, "the forgotten connector reports stopped");

            let st = start(None, 57839, &FrpcBinary::Explicit(script.clone())).expect("adopting start");
            assert!(st.running);
            assert_eq!(
                frpc_proc::read_pid_record(&frpc_config_path()).expect("record").pid,
                pid,
                "the same frpc is supervised — no respawn"
            );
            assert!(frpc_proc::pid_alive(pid), "adoption must not reap it");
            assert!(
                proxy_registered(),
                "the run's `start proxy success` line was replayed from the log"
            );
            let log = std::fs::read_to_string(frpc_log_path()).expect("log");
            assert_eq!(
                log.matches("start proxy success").count(),
                1,
                "exactly one frpc ever ran:\n{log}"
            );

            stop("test").expect("stop");
            let deadline = Instant::now() + Duration::from_secs(5);
            while frpc_proc::pid_alive(pid) {
                assert!(Instant::now() < deadline, "stop() must kill the adopted frpc");
                std::thread::sleep(Duration::from_millis(50));
            }
            assert!(
                frpc_proc::read_pid_record(&frpc_config_path()).is_none(),
                "a stopped tunnel leaves nothing to adopt"
            );
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// A changed config (here: a moved `localPort`) is never adopted: the
    /// old frpc is reaped and a fresh one dials with the new config.
    #[test]
    fn a_changed_config_is_not_adopted() {
        with_temp_home(|| {
            let dir = crate::test_env::unique_temp_path("adopt-frpc-changed");
            std::fs::create_dir_all(&dir).expect("mk dir");
            let script = registering_frpc(&dir);
            config::save(&TunnelConfig {
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                e2e: false,
                ..Default::default()
            })
            .expect("seed config");
            super::super::ingress::register_ensure_tunnel_ingress(Box::new(|| Ok(46124)));
            let _lease = crate::test_env::EnvVar::set(super::super::lease::LEASE_ENV_VAR, "off");
            start(None, 57839, &FrpcBinary::Explicit(script.clone())).expect("start");
            let old = hand_off_and_forget();

            // The ingress seam is first-write-wins, so move the port by
            // editing what the rendered config must match: a different
            // subdomain renders a different proxy name.
            config::update(|c| c.subdomain = "rosson2".to_string()).expect("edit config");
            start(None, 57839, &FrpcBinary::Explicit(script.clone())).expect("fresh start");
            let new = frpc_proc::read_pid_record(&frpc_config_path()).expect("record").pid;
            assert_ne!(new, old, "a changed config must spawn a fresh frpc");
            let deadline = Instant::now() + Duration::from_secs(5);
            while frpc_proc::pid_alive(old) {
                assert!(Instant::now() < deadline, "the stale frpc must be reaped");
                std::thread::sleep(Duration::from_millis(50));
            }
            stop("test").expect("stop");
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// R1 port freeze: `resolved_local_port` is captured once at start and
    /// reused on every multi-relay rewrite — rotation must not change
    /// localPort even as serverAddr flips.
    #[test]
    fn supervisor_rotation_keeps_frozen_local_port() {
        with_temp_home(|| {
            // E2E off with no ingress hook: start freezes the daemon port
            // it was given. Never inherit another test's stub.
            let _no_hook = super::super::ingress::TestIngressHook::none();
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
                // Rotation-timing independent of the DNS gate.
                failover: Some("legacy".to_string()),
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                e2e: false,
                ..Default::default()
            })
            .expect("seed two-relay config");

            // Pin an explicit non-default daemon port so a freeze bug would
            // be obvious (would rebind to 0 / missing / wrong port). No
            // ingress hook (another test's would answer its own port).
            super::super::ingress::clear_ensure_tunnel_ingress();
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

            stop("test").expect("stop");
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
        // Reserved, not listening (a dropped listener's port can be
        // re-bound by a parallel test before we probe it).
        let closed = crate::test_env::ClosedPort::new();
        let port = closed.port();
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

    /// The spec-example frpc TOML (was a print-only `#[ignore]`d
    /// diagnostic): pin its proxy shape. Token is a placeholder.
    #[test]
    fn spec_example_toml_has_the_proxy_shape() {
        let cfg = TunnelConfig {
            token: "REDACTED".to_string(),
            subdomain: "rosson".to_string(),
            local_port: Some(57839),
            ..Default::default()
        };
        let toml = super::super::render::render_frpc_toml(&cfg, 57839, false);
        assert!(toml.starts_with("# K2SO tunnel connector"), "{toml}");
        let proxy = toml.split("[[proxies]]\n").nth(1).expect("one proxy section");
        assert_eq!(
            proxy,
            "name = \"k2so-rosson\"\n\
             type = \"http\"\n\
             localIP = \"127.0.0.1\"\n\
             localPort = 57839\n\
             subdomain = \"rosson\"\n"
        );
        assert!(toml.contains("[metadatas]\ntoken = \"REDACTED\"\n"), "{toml}");
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
        stop("test").expect("stop live tunnel");
        assert!(!status().running);
    }
}
