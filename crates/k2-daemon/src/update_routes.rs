//! P3 — daemon-side remote self-UPDATE for the STANDALONE `k2-daemon`
//! binary (Shape B: the bare binary that P1/P2 produce + a supervisor
//! that respawns it on exit).
//!
//! GOAL: a remote (Owner/Admin over K2 Connect) or the local CLI can
//! drive the running daemon through:
//!
//!   check  → download+verify+stage → apply (atomic swap + restart)
//!          → health-check → rollback on failure
//!
//! ROUTES (registered in `routes::dispatcher`):
//!   - `POST /cli/daemon/update/check`  → compare running version to the
//!     published `daemon-latest.json`. Read-only.
//!   - `POST /cli/daemon/update/start`  → async job (blocking pool):
//!     download this platform's artifact + `.sig`, **verify minisign
//!     against the EMBEDDED pubkey (MANDATORY)**, verify sha256, stage.
//!   - `GET  /cli/daemon/update/status?job_id=` → job phase/progress.
//!   - `POST /cli/daemon/update/apply`  → back up the running binary,
//!     spawn a DETACHED swap/rollback helper, then trigger the P0
//!     graceful shutdown so the supervisor respawns the NEW binary.
//!
//! ── Shape A (macOS `.app`-bundle swap into /Applications) is a
//!    DOCUMENTED FOLLOW-UP, not built here. See `swap_shape_a_followup`. ──
//!
//! ── e2e-SMOKE-TEST-PENDING ──────────────────────────────────────────
//! The real download + atomic swap + supervisor relaunch + `/boot-status`
//! health-check + rollback are NOT exercised by the unit/integration
//! tests in this crate: they would mutate the on-disk binary, spawn a
//! detached process, and KILL the daemon. They are gated behind the SAME
//! `shutdown_tx == None` test seam as #659/#660 (`apply` returns its ack
//! and SKIPS the live shutdown trigger + helper spawn when `shutdown_tx`
//! is `None`). Everything that IS verifiable here is unit-tested:
//! manifest parse, version-compare / `available` decision, minisign
//! verify (good vs tampered, via the upstream test vector), sha256 check,
//! and the job state machine.

use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cli_response::CliResponse;

/// The minisign public key the daemon verifies update artifacts against.
///
/// This is the EXACT value of `plugins.updater.pubkey` in
/// `src-tauri/tauri.conf.json` — a base64 wrapper around the standard
/// 2-line minisign `.pub` file (`untrusted comment: …\n<key-b64>`). The
/// signing key NEVER lives in the daemon; this is verify-only. Embedding
/// it as a compile-time constant means an attacker can't point the daemon
/// at a manifest signed by some other key.
///
/// If `tauri.conf.json` rotates this key, update this constant in lockstep
/// (a follow-up could codegen it from the manifest at build time).
pub const UPDATER_PUBKEY_B64: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEU5MTExNDQ2RjY1RUJCMDUKUldRRnUxNzJSaFFSNlFCcXptaWoyRTlidERHaERXbXBkSCthaDEvTTRQbXVIUElOVVd2S0xmNm8K";

/// The marketing version this build reports + compares against the
/// manifest. Mirrors what `/boot-status` returns.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

// ─────────────────────────────────────────────────────────────────────
// Manifest (`daemon-latest.json`)
// ─────────────────────────────────────────────────────────────────────

/// One artifact entry in the manifest, keyed by `<os>-<arch>`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Artifact {
    /// Direct download URL for the `k2-daemon-<os>-<arch>` binary.
    pub url: String,
    /// Direct download URL for the detached minisign `.sig`.
    pub sig: String,
    /// Lowercase hex SHA-256 of the binary (independent integrity check
    /// layered on top of the signature).
    pub sha256: String,
}

/// `daemon-latest.json` — the release manifest the daemon fetches to
/// decide whether an update exists.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DaemonManifest {
    pub version: String,
    #[serde(default)]
    pub pub_date: String,
    /// Per-platform artifacts keyed by `<os>-<arch>` (e.g.
    /// `macos-aarch64`, `linux-x86_64`).
    pub artifacts: std::collections::HashMap<String, Artifact>,
    /// Optional human-readable release notes.
    #[serde(default)]
    pub notes: Option<String>,
}

impl DaemonManifest {
    /// Parse a manifest from raw bytes, returning a stable error string on
    /// malformed JSON.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(bytes).map_err(|e| format!("invalid daemon-latest.json: {e}"))
    }

    /// The `<os>-<arch>` artifact for THIS running platform, if the
    /// manifest publishes one.
    pub fn artifact_for_current_platform(&self) -> Option<&Artifact> {
        self.artifacts.get(&platform_key())
    }
}

/// The `<os>-<arch>` key for THIS build, matching the publisher's naming.
///
/// `os` ∈ macos | linux | windows ; `arch` ∈ aarch64 | x86_64 | … . We
/// translate Rust's `std::env::consts` to the publisher's vocabulary
/// (`macos` rather than rustc's `target_os = "macos"` which is already
/// "macos"; we normalize anyway so the contract is explicit).
pub fn platform_key() -> String {
    let os = match std::env::consts::OS {
        "macos" => "macos",
        "linux" => "linux",
        "windows" => "windows",
        other => other,
    };
    let arch = std::env::consts::ARCH; // "aarch64" | "x86_64" | …
    format!("{os}-{arch}")
}

// ─────────────────────────────────────────────────────────────────────
// Version compare / `available` decision
// ─────────────────────────────────────────────────────────────────────

/// Result of `POST /cli/daemon/update/check`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CheckResult {
    pub current: String,
    pub latest: String,
    /// True iff `latest` is a STRICTLY HIGHER semver than `current` AND a
    /// downloadable artifact exists for this platform.
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Install topology of THIS host (`standalone` | `bundled-app` |
    /// `unknown`) so the renderer can route the remote-update mechanism
    /// (Shape B binary swap vs Shape A app-updater trigger) and vary copy;
    /// `update/start` routes on it server-side. PRD §3.1. Always present.
    #[serde(rename = "installKind")]
    pub install_kind: String,
}

/// Compare two dotted version strings (`x.y.z`) numerically, longest-
/// wins on a prefix tie (`1.2.0` < `1.2.0.1`). Non-numeric components
/// compare as 0 so a malformed manifest can never report a spurious
/// upgrade. Returns `Ordering` of `a` vs `b`.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let pa: Vec<u64> = a.split('.').map(|c| c.parse::<u64>().unwrap_or(0)).collect();
    let pb: Vec<u64> = b.split('.').map(|c| c.parse::<u64>().unwrap_or(0)).collect();
    let n = pa.len().max(pb.len());
    for i in 0..n {
        let x = pa.get(i).copied().unwrap_or(0);
        let y = pb.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            Ordering::Equal => continue,
            ord => return ord,
        }
    }
    Ordering::Equal
}

/// Decide whether `manifest` offers an upgrade over `current`, building
/// the `CheckResult`.
///
/// `available` rules, keyed on `install_kind`:
///   - `"standalone"` (Shape B): true ONLY when the manifest version is
///     strictly newer AND this platform has a daemon artifact (you can't
///     swap-apply what you can't download).
///   - `"bundled-app"` (Shape A): true whenever the manifest version is
///     strictly newer — the app's OWN Tauri updater (not the daemon
///     artifact) performs the download, so a missing daemon artifact must
///     NOT suppress the offer.
///   - `"unknown"`: conservative — same as standalone (require an artifact).
pub fn decide_check(current: &str, manifest: &DaemonManifest, install_kind: &str) -> CheckResult {
    let newer = compare_versions(&manifest.version, current) == std::cmp::Ordering::Greater;
    let artifact = manifest.artifact_for_current_platform();
    let available = if install_kind == "bundled-app" {
        newer
    } else {
        newer && artifact.is_some()
    };
    CheckResult {
        current: current.to_string(),
        latest: manifest.version.clone(),
        available,
        notes: manifest.notes.clone(),
        url: artifact.map(|a| a.url.clone()),
        install_kind: install_kind.to_string(),
    }
}

// ─────────────────────────────────────────────────────────────────────
// Verification — minisign + sha256
// ─────────────────────────────────────────────────────────────────────

/// Verify `data` against a detached minisign signature `sig_str` using
/// the public key `pubkey_wrapped_b64` (the base64-wrapped `.pub` file,
/// e.g. [`UPDATER_PUBKEY_B64`]).
///
/// MANDATORY on the update path: a mismatch is a HARD abort — the caller
/// must take NO further action (no sha256 check, no staging, no swap).
///
/// Real minisign signatures are prehashed (BLAKE2b), so we verify with
/// `allow_legacy = false`. Returns `Ok(())` on a valid signature, or a
/// stable error string otherwise.
pub fn verify_minisign(
    pubkey_wrapped_b64: &str,
    sig_str: &str,
    data: &[u8],
) -> Result<(), String> {
    use base64::Engine as _;
    // The embedded pubkey is base64 OF the 2-line `.pub` file. Decode that
    // outer wrapper first, then hand the `.pub` text to PublicKey::decode.
    let pub_file = base64::engine::general_purpose::STANDARD
        .decode(pubkey_wrapped_b64.trim())
        .map_err(|e| format!("updater pubkey is not valid base64: {e}"))?;
    let pub_file = String::from_utf8(pub_file)
        .map_err(|e| format!("updater pubkey is not valid utf8: {e}"))?;
    let public_key = minisign_verify::PublicKey::decode(pub_file.trim())
        .map_err(|e| format!("updater pubkey decode failed: {e}"))?;
    let signature = minisign_verify::Signature::decode(sig_str.trim())
        .map_err(|e| format!("signature decode failed: {e}"))?;
    public_key
        .verify(data, &signature, false)
        .map_err(|e| format!("minisign verification failed: {e}"))
}

/// True iff the lowercase-hex SHA-256 of `data` equals `expected_hex`
/// (case-insensitive comparison). A second integrity layer on top of the
/// signature — catches a truncated download whose `.sig` somehow still
/// matched (it never would, but defense in depth is cheap).
pub fn verify_sha256(data: &[u8], expected_hex: &str) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let got = hasher.finalize();
    let got_hex = hex_encode(&got);
    got_hex.eq_ignore_ascii_case(expected_hex.trim())
}

/// Lowercase hex-encode bytes (no external hex crate dependency).
fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// ─────────────────────────────────────────────────────────────────────
// Job state machine
// ─────────────────────────────────────────────────────────────────────

/// Lifecycle phase of an update job. The ordered happy path is
/// downloading → verifying → staged → applying → restarting → done.
/// `failed` (with `error`) and `rolled-back` are terminal off-ramps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
// `Done` and `RolledBack` are TERMINAL phases the detached shell swap
// helper conceptually transitions a job to AFTER the daemon has exited —
// at which point the in-process job map no longer exists, so no Rust code
// writes them today. They remain part of the wire contract (asserted in
// `phase_wire_strings_match_contract`) and will be set once the helper
// reports back through a persisted job file (e2e-smoke-test-pending). Allow
// the dead-code warning rather than drop contract phases.
#[allow(dead_code)]
pub enum Phase {
    Downloading,
    Verifying,
    Staged,
    Applying,
    Restarting,
    Done,
    Failed,
    RolledBack,
}

impl Phase {
    /// The wire string for this phase (matches the `?job_id=` status
    /// contract: downloading|verifying|staged|applying|restarting|done|
    /// failed|rolled-back).
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Downloading => "downloading",
            Phase::Verifying => "verifying",
            Phase::Staged => "staged",
            Phase::Applying => "applying",
            Phase::Restarting => "restarting",
            Phase::Done => "done",
            Phase::Failed => "failed",
            Phase::RolledBack => "rolled-back",
        }
    }
}

/// A single update job's observable state. The download worker mutates it
/// through the phases; `GET …/status` snapshots it.
#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub job_id: String,
    pub phase: Phase,
    /// Target version this job is moving the daemon TO (from the manifest).
    pub target_version: String,
    /// 0.0–1.0 download progress when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    /// Bytes downloaded so far when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Absolute path the verified binary is staged at (set on `staged`).
    #[serde(skip)]
    pub staged_path: Option<PathBuf>,
}

impl Job {
    fn new(job_id: String, target_version: String) -> Self {
        Job {
            job_id,
            phase: Phase::Downloading,
            target_version,
            progress: None,
            bytes: None,
            error: None,
            staged_path: None,
        }
    }
}

/// Process-wide job registry. Keyed by `job_id`. A single in-flight
/// update at a time is the expected use, but the map supports more.
fn jobs() -> &'static StdMutex<std::collections::HashMap<String, Job>> {
    static JOBS: OnceLock<StdMutex<std::collections::HashMap<String, Job>>> = OnceLock::new();
    JOBS.get_or_init(|| StdMutex::new(std::collections::HashMap::new()))
}

/// Insert a fresh `downloading` job and return its id.
pub fn create_job(target_version: &str) -> String {
    let job_id = new_job_id();
    let job = Job::new(job_id.clone(), target_version.to_string());
    jobs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(job_id.clone(), job);
    job_id
}

/// Snapshot a job's state for the status route.
pub fn get_job(job_id: &str) -> Option<Job> {
    jobs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(job_id)
        .cloned()
}

/// Apply `f` to a job in place (phase transition / progress update). No-op
/// if the job_id is unknown.
pub fn update_job<F: FnOnce(&mut Job)>(job_id: &str, f: F) {
    if let Some(job) = jobs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(job_id)
    {
        f(job);
    }
}

/// Transition a job to `phase`.
fn set_phase(job_id: &str, phase: Phase) {
    update_job(job_id, |j| j.phase = phase);
}

/// Mark a job `failed` with an error message. Terminal.
fn fail_job(job_id: &str, err: impl Into<String>) {
    let err = err.into();
    // Surface the real failure reason server-side: the host log used to be
    // silent on update failures (the root cause of the 0.39.34 sig bug going
    // undiagnosed). This logs on EVERY failure path before the job flips.
    k2_core::log_debug!("[daemon] P3 update/download — job {job_id} FAILED: {err}");
    update_job(job_id, |j| {
        j.phase = Phase::Failed;
        j.error = Some(err);
    });
}

/// Generate a short, collision-resistant job id (random hex). No uuid dep
/// needed — 16 random bytes via getrandom.
fn new_job_id() -> String {
    let mut bytes = [0u8; 16];
    // getrandom is already a daemon dep; fall back to a timestamp-derived
    // id only if the OS RNG is unavailable (never expected on macOS/Linux).
    if getrandom::getrandom(&mut bytes).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        return format!("job-{nanos:x}");
    }
    hex_encode(&bytes)
}

#[cfg(test)]
fn clear_jobs_for_test() {
    jobs().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

// ─────────────────────────────────────────────────────────────────────
// Filesystem layout
// ─────────────────────────────────────────────────────────────────────

/// `~/.k2so/update/<job_id>/` — the staging dir for one job's downloaded
/// + verified artifacts. Falls back to the system temp dir if `$HOME`
/// can't be resolved.
fn job_stage_dir(job_id: &str) -> PathBuf {
    let base = dirs::home_dir()
        .map(|h| h.join(".k2").join("update"))
        .unwrap_or_else(|| std::env::temp_dir().join("k2so-update"));
    base.join(job_id)
}

/// `~/.k2so/update/backup-<ver>` — where `apply` copies the CURRENTLY
/// running binary before the swap, so the detached helper can restore it
/// if the new binary fails its health-check.
fn backup_path(version: &str) -> PathBuf {
    let base = dirs::home_dir()
        .map(|h| h.join(".k2").join("update"))
        .unwrap_or_else(|| std::env::temp_dir().join("k2so-update"));
    base.join(format!("backup-{version}"))
}

// ─────────────────────────────────────────────────────────────────────
// HTTP handlers
// ─────────────────────────────────────────────────────────────────────

/// `POST /cli/daemon/update/check` — fetch `daemon-latest.json`, compare
/// to the running version, report whether an update is available for this
/// platform. Read-only (downloads only the small JSON manifest).
///
/// The manifest URL is resolved from the embedded default (overridable via
/// the `K2SO_DAEMON_MANIFEST_URL` env for testing/self-hosting). Network
/// I/O runs on a blocking worker (the dispatcher spawns this on
/// `spawn_blocking`).
pub fn handle_check() -> CliResponse {
    let url = manifest_url();
    let bytes = match fetch_bytes(&url) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("fetch manifest: {e}")),
    };
    let manifest = match DaemonManifest::parse(&bytes) {
        Ok(m) => m,
        Err(e) => return CliResponse::bad_request(e),
    };
    let result = decide_check(current_version(), &manifest, crate::boot_status::install_kind());
    CliResponse::ok_json(serde_json::to_string(&result).unwrap_or_default())
}

/// `POST /cli/daemon/update/start` — body `{version?}`. Create a job,
/// kick off the download+verify+stage pipeline on a DETACHED blocking
/// task, and return `{job_id}` immediately. NEVER blocks the HTTP thread.
///
/// The pipeline:
///   1. fetch the manifest, resolve THIS platform's artifact (404 if the
///      requested/latest version has no artifact for us),
///   2. download the binary + `.sig`,
///   3. **verify minisign against [`UPDATER_PUBKEY_B64`] — MANDATORY**;
///      on mismatch the job goes `failed` and NOTHING else happens,
///   4. verify sha256,
///   5. write the binary into `~/.k2so/update/<job_id>/k2-daemon` (mode
///      0755) and mark the job `staged`.
/// `event_tx` is the daemon's `/events` broadcast sender, threaded in so a
/// `"bundled-app"` host can EMIT the `app:update-trigger` frame to its
/// co-located Tauri app (Shape A). `None` in the test harness ⇒ the Shape A
/// branch fails the job with the "app isn't running" reason (receiver_count
/// is 0), which is the correct observable behavior with no app attached.
pub fn handle_start(
    body: &[u8],
    event_tx: Option<std::sync::Arc<tokio::sync::broadcast::Sender<crate::events::WireEvent>>>,
) -> CliResponse {
    #[derive(Deserialize, Default)]
    struct Req {
        #[serde(default)]
        version: Option<String>,
    }
    let req: Req = if body.is_empty() {
        Req::default()
    } else {
        match serde_json::from_slice(body) {
            Ok(r) => r,
            Err(e) => return CliResponse::bad_request(format!("invalid body: {e}")),
        }
    };

    // ── Shape selector (0.39.35) ────────────────────────────────────────
    // A "bundled-app" host CANNOT swap its own binary (it's inside a
    // signed/notarized .app); it must remote-trigger its co-located Tauri
    // updater. Branch BEFORE the daemon-manifest fetch — the bundled path
    // doesn't use daemon-latest.json at all (the app has its OWN updater
    // feed); it just needs a job + a trigger.
    if crate::boot_status::install_kind() == "bundled-app" {
        return start_bundled_app_update(req.version, event_tx);
    }

    // ── Shape B (standalone) ────────────────────────────────────────────
    // Resolve the manifest synchronously enough to learn the target
    // version + reject "nothing to do" up front; the heavy download then
    // runs detached.
    let url = manifest_url();
    let bytes = match fetch_bytes(&url) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("fetch manifest: {e}")),
    };
    let manifest = match DaemonManifest::parse(&bytes) {
        Ok(m) => m,
        Err(e) => return CliResponse::bad_request(e),
    };
    // If the caller pinned a version it must match the manifest's (we only
    // publish one latest); otherwise use the manifest version.
    let target = req.version.unwrap_or_else(|| manifest.version.clone());
    if target != manifest.version {
        return CliResponse::bad_request(format!(
            "requested version {target} != published {}",
            manifest.version
        ));
    }
    let artifact = match manifest.artifact_for_current_platform() {
        Some(a) => a.clone(),
        None => {
            return CliResponse::bad_request(format!(
                "no artifact for this platform ({})",
                platform_key()
            ))
        }
    };

    let job_id = create_job(&target);

    // Detached worker: download + verify + stage. Runs on the blocking
    // pool so it NEVER ties up an accept-loop thread. The HTTP thread has
    // already returned `{job_id}` by the time this runs.
    let job_id_for_worker = job_id.clone();
    std::thread::spawn(move || {
        run_download_stage(&job_id_for_worker, &artifact);
    });

    CliResponse::ok_json(serde_json::json!({ "job_id": job_id }).to_string())
}

/// Shape A: remote-trigger the co-located Tauri app's OWN updater.
///
/// Flow:
///   1. Create the job (target version is best-effort — the app's updater
///      resolves the actual version from its own feed; we record what the
///      caller asked for, or the current version as a placeholder).
///   2. Confirm the app is RUNNING by checking the `/events` broadcast has
///      at least one subscriber (`receiver_count() > 0`). The app's
///      `daemon_events` subscriber holds exactly such a receiver. If it's
///      0, the app isn't open on this host → fail the job with a clear,
///      actionable message rather than emitting a trigger nobody hears.
///   3. Emit `WireEvent { event: "app:update-trigger", payload: {job_id} }`.
///      The app re-emits it through Tauri, runs its updater, and POSTs
///      phases back to `/cli/daemon/app-update/progress` (see
///      [`handle_app_update_progress`]) so `/status` reflects them.
///   4. Return `{job_id}` immediately — the HTTP thread NEVER blocks on the
///      app-side updater.
fn start_bundled_app_update(
    requested_version: Option<String>,
    event_tx: Option<std::sync::Arc<tokio::sync::broadcast::Sender<crate::events::WireEvent>>>,
) -> CliResponse {
    // We don't know the published app version here (the app owns its feed),
    // so record the caller's request or the current version as a placeholder
    // the status route can echo until the app reports its target.
    let target = requested_version.unwrap_or_else(|| current_version().to_string());
    let job_id = create_job(&target);

    let Some(tx) = event_tx else {
        // No broadcast sender (test harness / no app wiring) ⇒ the co-located
        // app definitionally isn't reachable. Surface the same actionable
        // error the receiver_count==0 path does.
        fail_job(&job_id, app_not_running_msg());
        return CliResponse::ok_json(serde_json::json!({ "job_id": job_id }).to_string());
    };

    if tx.receiver_count() == 0 {
        // No `/events` subscriber ⇒ K2SO.app isn't running on this host.
        // Fail the job (the remote client polling /status sees a real
        // reason) but still return {job_id} so the client can read it.
        fail_job(&job_id, app_not_running_msg());
        return CliResponse::ok_json(serde_json::json!({ "job_id": job_id }).to_string());
    }

    // Emit the trigger. The app's daemon_events subscriber re-emits it via
    // Tauri; the renderer drives its updater and POSTs phases back.
    let frame = crate::events::WireEvent {
        event: "app:update-trigger".to_string(),
        payload: serde_json::json!({ "job_id": job_id }),
    };
    let _ = tx.send(frame);

    CliResponse::ok_json(serde_json::json!({ "job_id": job_id }).to_string())
}

/// Actionable "the app isn't open" message surfaced on a bundled-app job
/// when no co-located app is listening to drive the Tauri updater.
fn app_not_running_msg() -> String {
    let host = std::env::var("K2SO_HOSTNAME")
        .ok()
        .or_else(|| hostname_best_effort())
        .unwrap_or_else(|| "this host".to_string());
    format!("K2.app isn't running on {host} — open it there, or update on the machine.")
}

/// Best-effort hostname for the app-not-running message. Falls back to
/// `None` (the caller substitutes "this host") rather than failing.
fn hostname_best_effort() -> Option<String> {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Map a wire phase string (sent by the app over
/// `/cli/daemon/app-update/progress`) to the [`Phase`] enum. Returns `None`
/// for an unrecognized string so the route can 400 a bad request.
pub fn phase_from_str(s: &str) -> Option<Phase> {
    match s {
        "downloading" => Some(Phase::Downloading),
        "verifying" => Some(Phase::Verifying),
        "staged" => Some(Phase::Staged),
        "applying" => Some(Phase::Applying),
        "restarting" => Some(Phase::Restarting),
        "done" => Some(Phase::Done),
        "failed" => Some(Phase::Failed),
        "rolled-back" => Some(Phase::RolledBack),
        _ => None,
    }
}

/// `POST /cli/daemon/app-update/progress` — body
/// `{ job_id, phase, progress?, error? }`. The co-located Tauri app calls
/// this at each step of its OWN updater (Shape A) so the existing
/// `/cli/daemon/update/status` poll surfaces app-side progress uniformly.
///
/// Validates the phase string (bad ⇒ 400) and the job_id (unknown ⇒ 400),
/// then maps phase→[`Phase`] and writes phase/progress/error onto the job.
pub fn handle_app_update_progress(body: &[u8]) -> CliResponse {
    #[derive(Deserialize)]
    struct Req {
        job_id: String,
        phase: String,
        #[serde(default)]
        progress: Option<f64>,
        #[serde(default)]
        error: Option<String>,
    }
    let req: Req = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("invalid body: {e}")),
    };
    let phase = match phase_from_str(&req.phase) {
        Some(p) => p,
        None => {
            return CliResponse::bad_request(format!("unknown phase '{}'", req.phase));
        }
    };
    if get_job(&req.job_id).is_none() {
        return CliResponse::bad_request(format!("unknown job_id {}", req.job_id));
    }
    update_job(&req.job_id, |j| {
        j.phase = phase;
        if let Some(p) = req.progress {
            j.progress = Some(p);
        }
        // Only overwrite error when the app actually reported one, so a
        // later non-failed phase doesn't clobber a prior message oddly;
        // the app sends `error` only with the "failed" phase.
        if req.error.is_some() {
            j.error = req.error.clone();
        }
    });
    CliResponse::ok_json(serde_json::json!({ "ok": true }).to_string())
}

/// The download → verify → stage pipeline for one job. Mutates the job's
/// phase as it goes; on ANY failure the job goes `failed` and the
/// pipeline aborts (no staging, no further action). Runs on a detached
/// worker thread spawned by [`handle_start`].
fn run_download_stage(job_id: &str, artifact: &Artifact) {
    set_phase(job_id, Phase::Downloading);
    k2_core::log_debug!(
        "[daemon] P3 update/download — job {job_id} downloading binary {}",
        artifact.url
    );
    let bin = match fetch_bytes(&artifact.url) {
        Ok(b) => b,
        Err(e) => return fail_job(job_id, format!("download binary: {e}")),
    };
    update_job(job_id, |j| {
        j.bytes = Some(bin.len() as u64);
        j.progress = Some(1.0);
    });
    k2_core::log_debug!(
        "[daemon] P3 update/download — job {job_id} downloading sig {}",
        artifact.sig
    );
    let sig = match fetch_bytes(&artifact.sig) {
        Ok(b) => b,
        Err(e) => return fail_job(job_id, format!("download sig: {e}")),
    };
    let sig_str = match String::from_utf8(sig) {
        Ok(s) => s,
        Err(e) => return fail_job(job_id, format!("sig not utf8: {e}")),
    };

    // ── MANDATORY minisign verify against the EMBEDDED pubkey ──
    // A mismatch is a HARD abort: no sha256, no staging, no swap.
    set_phase(job_id, Phase::Verifying);
    k2_core::log_debug!("[daemon] P3 update/download — job {job_id} verifying minisign");
    if let Err(e) = verify_minisign(UPDATER_PUBKEY_B64, &sig_str, &bin) {
        return fail_job(job_id, format!("signature verify FAILED — aborting: {e}"));
    }
    // sha256 is the second integrity layer; only reached AFTER the
    // signature already validated.
    k2_core::log_debug!("[daemon] P3 update/download — job {job_id} verifying sha256");
    if !verify_sha256(&bin, &artifact.sha256) {
        return fail_job(job_id, "sha256 mismatch — aborting");
    }

    // Write the verified binary into the job stage dir (0755).
    let dir = job_stage_dir(job_id);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return fail_job(job_id, format!("create stage dir: {e}"));
    }
    let staged = dir.join("k2-daemon");
    if let Err(e) = std::fs::write(&staged, &bin) {
        return fail_job(job_id, format!("write staged binary: {e}"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755));
    }
    update_job(job_id, |j| {
        j.phase = Phase::Staged;
        j.staged_path = Some(staged.clone());
    });
    k2_core::log_debug!(
        "[daemon] P3 update/download — job {job_id} staged at {}",
        staged.display()
    );
}

/// Build the status payload for `GET /cli/daemon/update/status?job_id=`.
/// 404s an unknown job_id.
pub fn handle_status(job_id: &str) -> CliResponse {
    match get_job(job_id) {
        Some(job) => {
            let body = serde_json::json!({
                "job_id": job.job_id,
                "phase": job.phase.as_str(),
                "progress": job.progress,
                "bytes": job.bytes,
                "error": job.error,
                "target_version": job.target_version,
            });
            CliResponse::ok_json(body.to_string())
        }
        None => CliResponse::not_found(),
    }
}

/// Outcome of [`prepare_apply`] — everything the (live) detached helper
/// needs, computed + validated on the HTTP thread so a bad request is
/// rejected with a clean error BEFORE any process work.
#[derive(Debug)]
pub struct ApplyPlan {
    pub job_id: String,
    pub target_version: String,
    pub staged_path: PathBuf,
    pub running_path: PathBuf,
    pub backup_path: PathBuf,
}

/// Validate an `apply` request and compute the swap plan WITHOUT touching
/// any process state. Separated from [`handle_apply`] so it's unit-
/// testable: it enforces `phase == staged`, that the staged file exists,
/// and resolves the running binary's own path.
///
/// Returns `Err(CliResponse)` (the response to send) on any precondition
/// failure.
pub fn prepare_apply(job_id: &str) -> Result<ApplyPlan, CliResponse> {
    let job = get_job(job_id)
        .ok_or_else(|| CliResponse::bad_request(format!("unknown job_id {job_id}")))?;
    if job.phase != Phase::Staged {
        return Err(CliResponse::bad_request(format!(
            "job {job_id} is '{}', not 'staged' — cannot apply",
            job.phase.as_str()
        )));
    }
    let staged_path = job
        .staged_path
        .clone()
        .ok_or_else(|| CliResponse::bad_request(format!("job {job_id} has no staged binary")))?;
    if !staged_path.exists() {
        return Err(CliResponse::bad_request(format!(
            "staged binary missing at {}",
            staged_path.display()
        )));
    }
    let running_path = std::env::current_exe()
        .map_err(|e| CliResponse::internal_error(format!("resolve current_exe: {e}")))?;
    Ok(ApplyPlan {
        job_id: job_id.to_string(),
        target_version: job.target_version.clone(),
        staged_path,
        running_path,
        backup_path: backup_path(&job.target_version),
    })
}

/// `POST /cli/daemon/update/apply` — body `{job_id}`. Only valid when the
/// job is `staged`. Backs up the running binary, spawns a DETACHED swap/
/// rollback helper, then triggers the P0 graceful shutdown so the
/// supervisor respawns the NEW binary.
///
/// SEAM (#651-parallel): `shutdown_tx` is `Some` only in the LIVE daemon.
/// In the test harness it is `None`, so this handler returns its 200 ack
/// and SKIPS the backup + helper spawn + shutdown trigger entirely — a
/// test NEVER swaps the binary, spawns the helper, or kills the process.
/// The real swap/restart/health-check/rollback is e2e-SMOKE-TEST-PENDING.
pub fn handle_apply(
    body: &[u8],
    shutdown_tx: Option<tokio::sync::broadcast::Sender<()>>,
) -> CliResponse {
    #[derive(Deserialize)]
    struct Req {
        job_id: String,
    }
    let req: Req = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("invalid body: {e}")),
    };

    let plan = match prepare_apply(&req.job_id) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    // ── TEST SEAM ──────────────────────────────────────────────────────
    // `None` ⇒ harness: assert the happy path (200 + would-apply) WITHOUT
    // backing up, spawning the helper, or killing the daemon.
    let Some(tx) = shutdown_tx else {
        return CliResponse::ok_json(
            serde_json::json!({
                "ok": true,
                "applying": true,
                "job_id": plan.job_id,
                "note": "test-seam: shutdown_tx None — no real swap/restart fired",
            })
            .to_string(),
        );
    };

    // ── LIVE PATH (e2e-smoke-test-pending) ─────────────────────────────
    // 1. Back up the running binary so the helper can roll back.
    if let Err(e) = std::fs::create_dir_all(plan.backup_path.parent().unwrap_or(Path::new("/"))) {
        return CliResponse::internal_error(format!("create backup dir: {e}"));
    }
    if let Err(e) = std::fs::copy(&plan.running_path, &plan.backup_path) {
        return CliResponse::internal_error(format!("back up running binary: {e}"));
    }
    set_phase(&plan.job_id, Phase::Applying);

    // 2. Spawn the DETACHED swap/rollback helper. It outlives the daemon:
    //    it waits for the daemon to exit, renames the staged binary onto
    //    the running path (same volume = atomic), lets the supervisor
    //    relaunch it, health-checks /boot-status, and rolls back on
    //    failure. See `spawn_swap_helper` for the contract.
    if let Err(e) = spawn_swap_helper(&plan) {
        // Couldn't even launch the helper — restore nothing changed yet,
        // just report. The binary on disk is still the old one.
        return CliResponse::internal_error(format!("spawn swap helper: {e}"));
    }

    // 3. Trigger the P0 graceful shutdown (detached, after the ack drains)
    //    so the supervisor respawns. The helper is already waiting.
    set_phase(&plan.job_id, Phase::Restarting);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        k2_core::log_debug!("[daemon] P3 update/apply — triggering graceful shutdown for swap");
        let _ = tx.send(());
    });

    CliResponse::ok_json(
        serde_json::json!({ "ok": true, "applying": true, "job_id": plan.job_id }).to_string(),
    )
}

/// Spawn the DETACHED swap/rollback helper process.
///
/// e2e-SMOKE-TEST-PENDING — this is only reached on the LIVE path (never
/// in tests, which take the `shutdown_tx == None` seam in [`handle_apply`]).
///
/// CONTRACT the helper implements (a tiny shell program, generated here so
/// it survives the daemon dying — same detached pattern as the app's
/// relaunch helper):
///   1. Poll until the running daemon's PID has exited (the graceful
///      shutdown is in flight).
///   2. `mv <staged> <running>` — an atomic rename on the SAME volume
///      (the stage dir is under `~/.k2so`, the running binary is wherever
///      the supervisor launches it; if they straddle volumes the helper
///      falls back to copy+fsync+rename-temp).
///   3. Let the supervisor (launchd KeepAlive / systemd Restart=always)
///      respawn the new binary; poll `/boot-status` until
///      `version == target && phase == ready`, up to a timeout.
///   4. On timeout/failure: restore `<backup>` onto `<running>`, let the
///      supervisor restart the OLD binary, and mark the job `rolled-back`.
///
/// We write the helper as a self-contained shell script into the job dir
/// and `setsid`-detach it so it has no controlling terminal and is
/// reparented to init when the daemon dies.
fn spawn_swap_helper(plan: &ApplyPlan) -> std::io::Result<()> {
    let script = render_swap_helper_script(plan);
    let helper = plan
        .staged_path
        .parent()
        .unwrap_or(Path::new("/tmp"))
        .join("swap-helper.sh");
    std::fs::write(&helper, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755))?;
    }
    // Detach: new session, no stdio inherited, so it survives the daemon's
    // exit. `setsid` reparents it to init.
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.arg(&helper)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                // Detach from the daemon's process group / session.
                libc::setsid();
                Ok(())
            });
        }
    }
    cmd.spawn()?;
    Ok(())
}

/// Render the detached swap/rollback shell helper for `plan`. Factored out
/// (pure string-building) so its shape is inspectable in a unit test
/// without spawning anything.
fn render_swap_helper_script(plan: &ApplyPlan) -> String {
    let pid = std::process::id();
    let staged = plan.staged_path.display();
    let running = plan.running_path.display();
    let backup = plan.backup_path.display();
    let port = daemon_port_hint();
    let target = &plan.target_version;
    // The script is intentionally dependency-light POSIX sh.
    format!(
        r#"#!/bin/sh
# P3 daemon swap/rollback helper — detached; survives the daemon exit.
# Generated by k2-daemon update/apply. e2e-smoke-test-pending.
set -u
DAEMON_PID={pid}
STAGED="{staged}"
RUNNING="{running}"
BACKUP="{backup}"
PORT="{port}"
TARGET="{target}"

# 1. Wait for the old daemon to exit (graceful shutdown in flight).
i=0
while kill -0 "$DAEMON_PID" 2>/dev/null; do
  i=$((i+1)); [ "$i" -ge 100 ] && break; sleep 0.1
done

# 2. Atomic swap (same-volume rename; fall back to cp if cross-volume).
if ! mv -f "$STAGED" "$RUNNING" 2>/dev/null; then
  cp -f "$STAGED" "$RUNNING" || exit 1
fi
chmod 0755 "$RUNNING" 2>/dev/null

# 3. Supervisor (launchd/systemd) respawns the new binary. Health-check.
ok=0
i=0
while [ "$i" -lt 60 ]; do
  i=$((i+1)); sleep 1
  body=$(curl -fsS "http://127.0.0.1:$PORT/boot-status" 2>/dev/null) || continue
  echo "$body" | grep -q "\"version\":\"$TARGET\"" || continue
  echo "$body" | grep -q "\"phase\":\"ready\"" || continue
  ok=1; break
done

# 4. Rollback on failure: restore the backup, let the supervisor restart.
if [ "$ok" -ne 1 ]; then
  cp -f "$BACKUP" "$RUNNING" 2>/dev/null
  chmod 0755 "$RUNNING" 2>/dev/null
fi
exit 0
"#,
    )
}

/// Best-effort read of the daemon's bound port from `~/.k2so/daemon.port`
/// for the helper's health-check. Defaults to a placeholder the helper
/// tolerates if unreadable (the health-check just won't match → rollback,
/// which is the safe direction).
fn daemon_port_hint() -> String {
    dirs::home_dir()
        .map(|h| h.join(".k2").join("daemon.port"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "0".to_string())
}

// ─────────────────────────────────────────────────────────────────────
// Network + config
// ─────────────────────────────────────────────────────────────────────

/// The `daemon-latest.json` URL. Overridable via `K2SO_DAEMON_MANIFEST_URL`
/// (self-hosting / tests). The default points at the public release
/// channel; P1 owns the canonical value, so this is a placeholder the
/// parent reconciles.
fn manifest_url() -> String {
    std::env::var("K2SO_DAEMON_MANIFEST_URL")
        .unwrap_or_else(|_| "https://github.com/Alakazam-211/K2SO/releases/latest/download/daemon-latest.json".to_string())
}

/// Blocking HTTP GET returning the body bytes. Reuses the daemon's
/// existing `reqwest` blocking client (already a dep for Claude Auth). A
/// 30s timeout bounds a hung connection.
fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        // GitHub release assets 302 → a CDN host; follow redirects so the
        // .sig / binary URLs resolve to their final location.
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| format!("build http client: {e}"))?;
    let resp = client
        .get(url)
        .send()
        .map_err(|e| format!("request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("http status {}", resp.status()));
    }
    resp.bytes()
        .map(|b| b.to_vec())
        .map_err(|e| format!("read body: {e}"))
}

// ─────────────────────────────────────────────────────────────────────
// Shape A follow-up (NOT built — documented seam)
// ─────────────────────────────────────────────────────────────────────

/// DOCUMENTED FOLLOW-UP — NOT implemented in P3.
///
/// Shape A is the macOS `.app`-bundle swap into `/Applications/K2.app`:
/// the daemon lives inside a signed+notarized bundle, so an update must
/// replace the WHOLE bundle (and may need elevation for `/Applications`),
/// re-staple notarization, and relaunch via the app's own updater rather
/// than a bare binary rename. That is materially different from Shape B's
/// single-binary swap implemented here and is explicitly OUT OF SCOPE for
/// P3. This stub marks the seam so a future PR has an obvious home.
#[allow(dead_code)]
pub fn swap_shape_a_followup() {
    unimplemented!("Shape A (.app-bundle swap into /Applications) is a documented P3 follow-up");
}

// ─────────────────────────────────────────────────────────────────────
// Unit tests — manifest parse, version decision, verify, state machine
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    // ── Manifest parse ──────────────────────────────────────────────

    const SAMPLE_MANIFEST: &str = r#"{
      "version": "0.40.0",
      "pub_date": "2026-06-05T00:00:00Z",
      "notes": "shiny new daemon",
      "artifacts": {
        "macos-aarch64": {
          "url": "https://releases.k2.dev/k2-daemon-macos-aarch64",
          "sig": "https://releases.k2.dev/k2-daemon-macos-aarch64.sig",
          "sha256": "abc123"
        },
        "linux-x86_64": {
          "url": "https://releases.k2.dev/k2-daemon-linux-x86_64",
          "sig": "https://releases.k2.dev/k2-daemon-linux-x86_64.sig",
          "sha256": "def456"
        }
      }
    }"#;

    #[test]
    fn manifest_parses_full_shape() {
        let m = DaemonManifest::parse(SAMPLE_MANIFEST.as_bytes()).expect("parse");
        assert_eq!(m.version, "0.40.0");
        assert_eq!(m.notes.as_deref(), Some("shiny new daemon"));
        assert_eq!(m.artifacts.len(), 2);
        let a = m.artifacts.get("macos-aarch64").expect("macos artifact");
        assert_eq!(a.sha256, "abc123");
        assert!(a.url.ends_with("k2-daemon-macos-aarch64"));
        assert!(a.sig.ends_with(".sig"));
    }

    #[test]
    fn manifest_sig_is_a_url_not_inline_signature() {
        // CONTRACT LOCK (0.39.34 root-cause regression): the daemon
        // downloads `artifact.sig` as a URL (fetch_bytes). The manifest
        // MUST therefore carry a URL in `sig`, never the inline base64
        // minisign blob. release.sh once wrote the inline content here,
        // which the downloader handed to reqwest → invalid-URL builder
        // error → every self-update failed at "download sig". If a future
        // change reverts to inline, this fails.
        let m = DaemonManifest::parse(SAMPLE_MANIFEST.as_bytes()).expect("parse");
        for (key, a) in &m.artifacts {
            assert!(
                a.sig.starts_with("http://") || a.sig.starts_with("https://"),
                "artifact {key} sig must be a URL (downloaded by fetch_bytes), got: {}",
                a.sig
            );
            assert!(
                a.sig.ends_with(".sig"),
                "artifact {key} sig URL should point at the .sig asset, got: {}",
                a.sig
            );
        }
    }

    #[test]
    fn manifest_rejects_malformed_json() {
        let err = DaemonManifest::parse(b"{not json").unwrap_err();
        assert!(err.contains("invalid daemon-latest.json"), "err={err}");
    }

    #[test]
    fn manifest_notes_optional() {
        let m = DaemonManifest::parse(
            br#"{"version":"1.0.0","artifacts":{}}"#,
        )
        .expect("parse minimal");
        assert!(m.notes.is_none());
        assert!(m.artifacts.is_empty());
    }

    // ── Version compare ─────────────────────────────────────────────

    #[test]
    fn compare_versions_numeric_ordering() {
        assert_eq!(compare_versions("0.39.32", "0.40.0"), Ordering::Less);
        assert_eq!(compare_versions("0.40.0", "0.39.32"), Ordering::Greater);
        assert_eq!(compare_versions("1.2.3", "1.2.3"), Ordering::Equal);
        // numeric, not lexicographic: 0.39.9 < 0.39.10
        assert_eq!(compare_versions("0.39.9", "0.39.10"), Ordering::Less);
        // longest-wins on prefix tie
        assert_eq!(compare_versions("1.2.0", "1.2.0.1"), Ordering::Less);
    }

    #[test]
    fn compare_versions_malformed_components_are_zero() {
        // A malformed component must never spuriously report an upgrade.
        assert_eq!(compare_versions("1.x.3", "1.0.3"), Ordering::Equal);
        assert_eq!(compare_versions("garbage", "0.0.0"), Ordering::Equal);
    }

    // ── decide_check `available` logic ──────────────────────────────

    #[test]
    fn decide_check_available_when_newer_and_artifact_present() {
        // Build a manifest whose only artifact is for THIS platform so the
        // decision is deterministic on whatever host runs the test.
        let mut artifacts = std::collections::HashMap::new();
        artifacts.insert(
            platform_key(),
            Artifact {
                url: "https://x/bin".into(),
                sig: "https://x/bin.sig".into(),
                sha256: "00".into(),
            },
        );
        let m = DaemonManifest {
            version: "999.0.0".into(),
            pub_date: String::new(),
            artifacts,
            notes: Some("n".into()),
        };
        let r = decide_check("0.39.0", &m, "standalone");
        assert!(r.available, "newer + artifact ⇒ available");
        assert_eq!(r.latest, "999.0.0");
        assert_eq!(r.url.as_deref(), Some("https://x/bin"));
        assert_eq!(r.notes.as_deref(), Some("n"));
        assert_eq!(r.install_kind, "standalone");
    }

    #[test]
    fn decide_check_not_available_when_same_or_older() {
        let mut artifacts = std::collections::HashMap::new();
        artifacts.insert(
            platform_key(),
            Artifact {
                url: "https://x/bin".into(),
                sig: "https://x/bin.sig".into(),
                sha256: "00".into(),
            },
        );
        let m = DaemonManifest {
            version: "0.0.1".into(),
            pub_date: String::new(),
            artifacts,
            notes: None,
        };
        // current is way newer than manifest ⇒ not available
        let r = decide_check("99.0.0", &m, "standalone");
        assert!(!r.available, "older manifest ⇒ not available");
    }

    #[test]
    fn decide_check_not_available_when_no_artifact_for_platform() {
        // Newer version but the ONLY artifact is for a bogus platform.
        let mut artifacts = std::collections::HashMap::new();
        artifacts.insert(
            "someos-somearch".into(),
            Artifact {
                url: "https://x/bin".into(),
                sig: "https://x/bin.sig".into(),
                sha256: "00".into(),
            },
        );
        let m = DaemonManifest {
            version: "999.0.0".into(),
            pub_date: String::new(),
            artifacts,
            notes: None,
        };
        let r = decide_check("0.1.0", &m, "standalone");
        assert!(
            !r.available,
            "no artifact for this platform ⇒ not available even though newer (standalone)"
        );
        assert!(r.url.is_none());

        // BUT a bundled-app host updates via its own Tauri updater, so a
        // missing daemon artifact must NOT suppress the offer — newer alone
        // is enough.
        let r_bundled = decide_check("0.1.0", &m, "bundled-app");
        assert!(
            r_bundled.available,
            "bundled-app: newer ⇒ available even with no daemon artifact"
        );
        assert_eq!(r_bundled.install_kind, "bundled-app");
        assert!(r_bundled.url.is_none(), "still no daemon artifact url");
    }

    // ── sha256 ──────────────────────────────────────────────────────

    #[test]
    fn sha256_matches_known_vector() {
        // SHA-256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(verify_sha256(b"abc", expected));
        assert!(verify_sha256(b"abc", &expected.to_uppercase()), "case-insensitive");
        assert!(!verify_sha256(b"abcd", expected), "different data must fail");
        assert!(!verify_sha256(b"abc", "deadbeef"), "wrong hash must fail");
    }

    // ── minisign verify (good vs tampered) ──────────────────────────
    //
    // Uses the upstream `minisign-verify` PREHASHED test vector: pubkey +
    // a real prehashed signature over b"test". We base64-WRAP the pubkey
    // the same way tauri.conf.json wraps the production key so the test
    // exercises the exact decode path `verify_minisign` uses in prod.

    /// The upstream prehashed test pubkey (bare base64 key line).
    const TEST_PUBKEY_BARE: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    /// A real prehashed minisign signature over the bytes b"test".
    const TEST_SIG_PREHASHED: &str = "untrusted comment: signature from minisign secret key\nRUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\ntrusted comment: timestamp:1556193335\tfile:test\ny/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg==";

    /// Wrap a bare minisign key the way tauri.conf.json does: build the
    /// 2-line `.pub` file then base64 the whole thing.
    fn wrap_pubkey(bare_key: &str) -> String {
        use base64::Engine as _;
        let pub_file = format!("untrusted comment: minisign public key\n{bare_key}\n");
        base64::engine::general_purpose::STANDARD.encode(pub_file)
    }

    #[test]
    fn minisign_good_signature_passes() {
        let wrapped = wrap_pubkey(TEST_PUBKEY_BARE);
        verify_minisign(&wrapped, TEST_SIG_PREHASHED, b"test")
            .expect("valid prehashed signature over b\"test\" must verify");
    }

    #[test]
    fn minisign_tampered_data_fails() {
        let wrapped = wrap_pubkey(TEST_PUBKEY_BARE);
        // Same signature, but the data was tampered (b"Test" != b"test").
        let err = verify_minisign(&wrapped, TEST_SIG_PREHASHED, b"Test")
            .expect_err("tampered data must FAIL verification");
        assert!(
            err.contains("verification failed") || err.contains("verify"),
            "err should be a verify failure: {err}"
        );
    }

    #[test]
    fn minisign_wrong_key_fails() {
        // The PRODUCTION embedded key (different key id than the test
        // signature) must reject the test signature.
        let err = verify_minisign(UPDATER_PUBKEY_B64, TEST_SIG_PREHASHED, b"test")
            .expect_err("signature from a different key must FAIL");
        assert!(!err.is_empty(), "expected a non-empty error");
    }

    #[test]
    fn minisign_garbage_signature_is_error_not_panic() {
        let wrapped = wrap_pubkey(TEST_PUBKEY_BARE);
        let err = verify_minisign(&wrapped, "not a signature", b"test")
            .expect_err("garbage sig must be a clean Err");
        assert!(err.contains("signature decode failed"), "err={err}");
    }

    #[test]
    fn minisign_embedded_prod_pubkey_decodes() {
        // The embedded production pubkey must at least DECODE (wrong key id
        // for the test sig, but the base64-wrap + .pub decode path works).
        use base64::Engine as _;
        let raw = base64::engine::general_purpose::STANDARD
            .decode(UPDATER_PUBKEY_B64)
            .expect("embedded pubkey is valid base64");
        let s = String::from_utf8(raw).expect("utf8");
        minisign_verify::PublicKey::decode(s.trim())
            .expect("embedded pubkey decodes as a minisign .pub");
    }

    // ── Job state machine ───────────────────────────────────────────

    #[test]
    fn job_lifecycle_create_get_update() {
        clear_jobs_for_test();
        let id = create_job("0.40.0");
        let j = get_job(&id).expect("job exists after create");
        assert_eq!(j.phase, Phase::Downloading);
        assert_eq!(j.target_version, "0.40.0");

        set_phase(&id, Phase::Verifying);
        assert_eq!(get_job(&id).unwrap().phase, Phase::Verifying);

        update_job(&id, |j| {
            j.phase = Phase::Staged;
            j.staged_path = Some(PathBuf::from("/tmp/k2-daemon"));
        });
        let j = get_job(&id).unwrap();
        assert_eq!(j.phase, Phase::Staged);
        assert_eq!(j.staged_path, Some(PathBuf::from("/tmp/k2-daemon")));

        fail_job(&id, "boom");
        let j = get_job(&id).unwrap();
        assert_eq!(j.phase, Phase::Failed);
        assert_eq!(j.error.as_deref(), Some("boom"));
    }

    #[test]
    fn get_unknown_job_is_none() {
        clear_jobs_for_test();
        assert!(get_job("nope").is_none());
    }

    // ── Shape A: phase-string → Phase mapping (app-update/progress) ──

    #[test]
    fn phase_from_str_maps_all_contract_strings() {
        assert_eq!(phase_from_str("downloading"), Some(Phase::Downloading));
        assert_eq!(phase_from_str("verifying"), Some(Phase::Verifying));
        assert_eq!(phase_from_str("staged"), Some(Phase::Staged));
        assert_eq!(phase_from_str("applying"), Some(Phase::Applying));
        assert_eq!(phase_from_str("restarting"), Some(Phase::Restarting));
        assert_eq!(phase_from_str("done"), Some(Phase::Done));
        assert_eq!(phase_from_str("failed"), Some(Phase::Failed));
        assert_eq!(phase_from_str("rolled-back"), Some(Phase::RolledBack));
    }

    #[test]
    fn phase_from_str_rejects_unknown() {
        assert_eq!(phase_from_str("nope"), None);
        assert_eq!(phase_from_str(""), None);
        // Round-trips with as_str for the well-known phases.
        for p in [
            Phase::Downloading,
            Phase::Verifying,
            Phase::Staged,
            Phase::Applying,
            Phase::Restarting,
            Phase::Done,
            Phase::Failed,
            Phase::RolledBack,
        ] {
            assert_eq!(phase_from_str(p.as_str()), Some(p));
        }
    }

    // ── Shape A: app-update/progress route updates the job ──────────────

    #[test]
    fn app_update_progress_updates_phase_and_progress() {
        clear_jobs_for_test();
        let id = create_job("0.40.0");
        let body = serde_json::json!({
            "job_id": id,
            "phase": "downloading",
            "progress": 0.5,
        })
        .to_string();
        let resp = handle_app_update_progress(body.as_bytes());
        assert!(resp.status.starts_with("200"), "status={}", resp.status);
        let j = get_job(&id).unwrap();
        assert_eq!(j.phase, Phase::Downloading);
        assert_eq!(j.progress, Some(0.5));
    }

    #[test]
    fn app_update_progress_records_error_on_failed() {
        clear_jobs_for_test();
        let id = create_job("0.40.0");
        let body = serde_json::json!({
            "job_id": id,
            "phase": "failed",
            "error": "updater feed 404",
        })
        .to_string();
        let resp = handle_app_update_progress(body.as_bytes());
        assert!(resp.status.starts_with("200"), "status={}", resp.status);
        let j = get_job(&id).unwrap();
        assert_eq!(j.phase, Phase::Failed);
        assert_eq!(j.error.as_deref(), Some("updater feed 404"));
    }

    #[test]
    fn app_update_progress_rejects_bad_phase() {
        clear_jobs_for_test();
        let id = create_job("0.40.0");
        let body = serde_json::json!({ "job_id": id, "phase": "bogus" }).to_string();
        let resp = handle_app_update_progress(body.as_bytes());
        assert!(resp.status.starts_with("400"), "status={}", resp.status);
        assert!(resp.body.contains("unknown phase"), "body={}", resp.body);
        // The job's phase must NOT have changed off its initial downloading.
        assert_eq!(get_job(&id).unwrap().phase, Phase::Downloading);
    }

    #[test]
    fn app_update_progress_rejects_unknown_job() {
        clear_jobs_for_test();
        let body = serde_json::json!({ "job_id": "ghost", "phase": "staged" }).to_string();
        let resp = handle_app_update_progress(body.as_bytes());
        assert!(resp.status.starts_with("400"), "status={}", resp.status);
        assert!(resp.body.contains("unknown job_id"), "body={}", resp.body);
    }

    #[test]
    fn app_update_progress_rejects_bad_body() {
        let resp = handle_app_update_progress(b"{not json");
        assert!(resp.status.starts_with("400"), "status={}", resp.status);
    }

    // ── Shape A: start_bundled_app_update — no app listening ─────────────

    #[test]
    fn bundled_start_fails_job_when_no_event_tx() {
        clear_jobs_for_test();
        // None ⇒ no broadcast sender at all (test harness): the co-located
        // app is definitionally unreachable, so the job must FAIL with the
        // actionable "app isn't running" reason rather than hang.
        let resp = start_bundled_app_update(Some("0.40.0".into()), None);
        assert!(resp.status.starts_with("200"), "status={}", resp.status);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        let id = v["job_id"].as_str().expect("job_id");
        let j = get_job(id).unwrap();
        assert_eq!(j.phase, Phase::Failed);
        assert!(
            j.error.as_deref().unwrap_or_default().contains("isn't running"),
            "error should explain app not running: {:?}",
            j.error
        );
    }

    #[test]
    fn bundled_start_fails_job_when_no_subscribers() {
        clear_jobs_for_test();
        // A sender with ZERO receivers ⇒ the app's /events subscriber isn't
        // attached ⇒ same actionable failure.
        let (tx, _) = tokio::sync::broadcast::channel::<crate::events::WireEvent>(4);
        let tx = std::sync::Arc::new(tx);
        let resp = start_bundled_app_update(None, Some(tx));
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        let id = v["job_id"].as_str().expect("job_id");
        let j = get_job(id).unwrap();
        assert_eq!(j.phase, Phase::Failed);
        assert!(j.error.as_deref().unwrap_or_default().contains("isn't running"));
    }

    #[test]
    fn bundled_start_emits_trigger_when_app_listening() {
        clear_jobs_for_test();
        // A live subscriber ⇒ the app is "running": the job is NOT failed and
        // an app:update-trigger frame carrying the job_id is broadcast.
        let (tx, mut rx) = tokio::sync::broadcast::channel::<crate::events::WireEvent>(4);
        let tx = std::sync::Arc::new(tx);
        let resp = start_bundled_app_update(Some("9.9.9".into()), Some(tx));
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        let id = v["job_id"].as_str().expect("job_id").to_string();
        // Job stays in its initial (downloading) phase — the app drives it.
        assert_eq!(get_job(&id).unwrap().phase, Phase::Downloading);
        let frame = rx.try_recv().expect("trigger frame broadcast");
        assert_eq!(frame.event, "app:update-trigger");
        assert_eq!(frame.payload["job_id"].as_str(), Some(id.as_str()));
    }

    #[test]
    fn phase_wire_strings_match_contract() {
        assert_eq!(Phase::Downloading.as_str(), "downloading");
        assert_eq!(Phase::Verifying.as_str(), "verifying");
        assert_eq!(Phase::Staged.as_str(), "staged");
        assert_eq!(Phase::Applying.as_str(), "applying");
        assert_eq!(Phase::Restarting.as_str(), "restarting");
        assert_eq!(Phase::Done.as_str(), "done");
        assert_eq!(Phase::Failed.as_str(), "failed");
        assert_eq!(Phase::RolledBack.as_str(), "rolled-back");
    }

    // ── prepare_apply preconditions ─────────────────────────────────

    #[test]
    fn prepare_apply_rejects_unknown_job() {
        clear_jobs_for_test();
        let err = prepare_apply("ghost").expect_err("unknown job must reject");
        assert!(err.status.starts_with("400"), "status={}", err.status);
        assert!(err.body.contains("unknown job_id"), "body={}", err.body);
    }

    #[test]
    fn prepare_apply_rejects_non_staged_job() {
        clear_jobs_for_test();
        let id = create_job("0.40.0"); // phase = Downloading, not Staged
        let err = prepare_apply(&id).expect_err("non-staged job must reject");
        assert!(err.status.starts_with("400"), "status={}", err.status);
        assert!(
            err.body.contains("not 'staged'"),
            "body should explain staged precondition: {}",
            err.body
        );
    }

    #[test]
    fn prepare_apply_rejects_missing_staged_file() {
        clear_jobs_for_test();
        let id = create_job("0.40.0");
        update_job(&id, |j| {
            j.phase = Phase::Staged;
            j.staged_path = Some(PathBuf::from("/no/such/staged/k2-daemon"));
        });
        let err = prepare_apply(&id).expect_err("missing staged file must reject");
        assert!(err.body.contains("staged binary missing"), "body={}", err.body);
    }

    #[test]
    fn prepare_apply_accepts_staged_job_with_existing_file() {
        clear_jobs_for_test();
        // Create a real staged file in a tempdir so the precondition passes.
        let dir = std::env::temp_dir().join(format!("k2so-p3-prep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mk tmp");
        let staged = dir.join("k2-daemon");
        std::fs::write(&staged, b"#!fake binary").expect("write staged");
        let id = create_job("0.40.0");
        update_job(&id, |j| {
            j.phase = Phase::Staged;
            j.staged_path = Some(staged.clone());
        });
        let plan = match prepare_apply(&id) {
            Ok(p) => p,
            Err(resp) => panic!("staged + existing file should plan; got {}", resp.status),
        };
        assert_eq!(plan.target_version, "0.40.0");
        assert_eq!(plan.staged_path, staged);
        assert!(
            plan.backup_path.to_string_lossy().contains("backup-0.40.0"),
            "backup path: {}",
            plan.backup_path.display()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── handle_apply test seam (None shutdown_tx ⇒ no swap/restart) ──

    #[test]
    fn handle_apply_none_seam_acks_without_firing() {
        clear_jobs_for_test();
        let dir = std::env::temp_dir().join(format!("k2so-p3-apply-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mk tmp");
        let staged = dir.join("k2-daemon");
        std::fs::write(&staged, b"#!fake binary").expect("write staged");
        let id = create_job("0.40.0");
        update_job(&id, |j| {
            j.phase = Phase::Staged;
            j.staged_path = Some(staged.clone());
        });
        let body = serde_json::json!({ "job_id": id }).to_string();
        // None ⇒ test seam: 200 ack, NO real swap/helper/shutdown.
        let resp = handle_apply(body.as_bytes(), None);
        assert!(resp.status.starts_with("200"), "status={}", resp.status);
        assert!(resp.body.contains("\"applying\":true"), "body={}", resp.body);
        assert!(resp.body.contains("test-seam"), "body should note the seam: {}", resp.body);
        // Phase did NOT advance to applying/restarting — the seam skipped it.
        assert_eq!(
            get_job(&id).unwrap().phase,
            Phase::Staged,
            "None seam must not advance the phase"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn handle_apply_rejects_bad_body() {
        let resp = handle_apply(b"{not json", None);
        assert!(resp.status.starts_with("400"), "status={}", resp.status);
    }

    // ── swap helper script shape (no spawn) ─────────────────────────

    #[test]
    fn swap_helper_script_has_expected_shape() {
        let plan = ApplyPlan {
            job_id: "abc".into(),
            target_version: "0.40.0".into(),
            staged_path: PathBuf::from("/home/u/.k2so/update/abc/k2-daemon"),
            running_path: PathBuf::from("/usr/local/bin/k2-daemon"),
            backup_path: PathBuf::from("/home/u/.k2so/update/backup-0.40.0"),
        };
        let s = render_swap_helper_script(&plan);
        assert!(s.starts_with("#!/bin/sh"), "shebang");
        assert!(s.contains("mv -f"), "atomic rename");
        assert!(s.contains("/boot-status"), "health-check endpoint");
        // The target version is bound to $TARGET and the health-check greps
        // for `"version":"$TARGET"` + `"phase":"ready"`.
        assert!(s.contains("TARGET=\"0.40.0\""), "target version bound");
        assert!(s.contains("\\\"version\\\":\\\"$TARGET\\\""), "greps version==target");
        assert!(s.contains("\\\"phase\\\":\\\"ready\\\""), "waits for ready");
        assert!(s.contains("cp -f \"$BACKUP\""), "rollback restores backup");
        assert!(s.contains("/usr/local/bin/k2-daemon"), "running path");
    }
}
