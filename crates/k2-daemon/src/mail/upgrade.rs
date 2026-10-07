//! `k2 hostmail upgrade` — the explicit Stalwart upgrade
//! (prd-hostmail-calendars-v1 S1: CAL7, CAL13–CAL15, CAL40, IT5, IT6).
//!
//! Owner-only, POST `/cli/mail/server/upgrade`. NEVER automatic: nothing
//! at boot, on a daemon update, or in enable calls into this module.
//!
//! **Preflight** (read-only; any blocker refuses with no side effect):
//! hosted mail installed and enable finished · installed == pin → no-op ·
//! installed must be [`STALWART_PREVIOUS_VERSION`] (the only version this
//! build can roll back to) · no interrupted / failed-rollback upgrade
//! awaiting the owner · no enable / upgrade / Maildir import running ·
//! the mail helper installed, protocol ≥ [`UPGRADE_MIN_PROTOCOL`], built
//! for exactly this pin + previous (its `version` verb; else the root
//! `install_command()`, CAL40) · free space ≥ data + config + 1 GiB
//! (helper `usage`, counted on the snapshot's filesystem, the old
//! snapshot counted as free since it is replaced).
//!
//! **Sequence** (background thread; progress in
//! `mail_server.upgrade_progress_json`, polled by `/cli/mail/status`):
//! download + sha-verify the pin AND the previous tarball (Stalwart still
//! serving) → stop (helper `systemctl stop stalwart`) → helper
//! `snapshot-data` → `install-bin` (pin) → start through the IMAP-listener
//! restart doors ([`super::imap_listeners::restart_once`]: helper
//! `systemctl restart stalwart`, else plain `sudo -n systemctl restart`)
//! → health (unit active + JMAP `ping()` + IMAP greeting). Success records
//! `installed_version`. A box that was not running before is stopped
//! again afterwards (CAL15: works from stopped / disabled / error).
//!
//! **Failure**: before any change (download, stop, snapshot) → `aborted`,
//! the old binary is started again if it was running. After the snapshot
//! → stop → helper `restore-data` → `install-bin` (previous) → start +
//! health if it was running → `rolled_back`; any rollback step failing →
//! `rollback_failed`, status `error`, exact manual steps, and the next
//! upgrade is blocked until `--acknowledge-failed`. Never `hostmail
//! disable/enable`; never re-runs a Maildir import.

use std::sync::atomic::Ordering;

use super::helper::{self, SNAPSHOT_FREE_MARGIN, UPGRADE_MIN_PROTOCOL};
use super::imap_listeners::{self, RestartDoors, RestartReport};
use super::supervisor::{
    self, artifact_for_arch, previous_artifact_for_arch, tarball_url_for, StalwartArtifact,
    STALWART_BIN, STALWART_CONFIG, STALWART_PINNED_VERSION, STALWART_PREVIOUS_VERSION,
    STALWART_SNAPSHOT_DIR, STALWART_SNAPSHOT_MARKER, STALWART_UNIT,
};
use super::sysops::SystemOps;

/// JMAP ping attempts after the start (Stalwart may open a large store
/// slowly after a version change), [`PING_INTERVAL_MS`] apart.
pub const PING_ATTEMPTS: u32 = 30;
pub const PING_INTERVAL_MS: u64 = 2_000;
/// IMAP greeting attempts after the ping succeeded.
pub const GREET_ATTEMPTS: u32 = 3;
/// Connect / read timeout of one IMAP greeting.
pub const GREET_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// Copy speed the outage estimate assumes (local SSD, many small RocksDB
/// files: conservative).
pub const COPY_BYTES_PER_SEC: u64 = 100 * 1024 * 1024;

const PROGRESS_COL: &str = "upgrade_progress_json";

/// What the owner asked for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UpgradeRequest {
    pub dry_run: bool,
    /// Clear a `rollback_failed` / interrupted record after the manual
    /// repair. Does NOT upgrade in the same call.
    pub acknowledge_failed: bool,
}

/// Route-facing answer of [`start_live`].
#[derive(Debug, Clone, PartialEq)]
pub enum UpgradeStart {
    /// Installed == pin.
    Noop(serde_json::Value),
    /// `--dry-run`: the plan (+ blockers); nothing changed.
    DryRun(serde_json::Value),
    /// A blocker: nothing changed.
    Refused {
        code: &'static str,
        hint: String,
        body: serde_json::Value,
    },
    /// Running in the background; poll `/cli/mail/status` → `upgrade`.
    Started(serde_json::Value),
    /// `--acknowledge-failed` cleared the block.
    Acknowledged(serde_json::Value),
}

/// One preflight refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocker {
    pub code: &'static str,
    pub hint: String,
}

impl Blocker {
    fn new(code: &'static str, hint: impl Into<String>) -> Self {
        Self {
            code,
            hint: hint.into(),
        }
    }
    fn json(&self) -> serde_json::Value {
        serde_json::json!({ "code": self.code, "hint": self.hint })
    }
}

/// In-memory facts preflight reads (globals in production; explicit in
/// tests so they never race other tests' latches).
#[derive(Debug, Clone, Copy, Default)]
pub struct Runtime {
    pub enable_busy: bool,
    pub upgrade_busy: bool,
    pub imports_running: usize,
}

impl Runtime {
    pub fn live() -> Self {
        Self {
            enable_busy: supervisor::enable_running().load(Ordering::SeqCst),
            upgrade_busy: supervisor::upgrade_running().load(Ordering::SeqCst),
            imports_running: super::import::imports_running(),
        }
    }
}

/// Everything preflight learned. Fields are `None` when a blocker stopped
/// the check that would fill them.
#[derive(Debug, Clone)]
pub struct Plan {
    pub from: Option<String>,
    pub to: &'static str,
    pub prior_status: Option<String>,
    pub was_running: bool,
    pub target: Option<&'static StalwartArtifact>,
    pub previous: Option<&'static StalwartArtifact>,
    pub helper: Option<serde_json::Value>,
    pub usage: Option<serde_json::Value>,
}

impl Plan {
    fn usage_u64(&self, key: &str) -> Option<u64> {
        self.usage.as_ref().and_then(|u| u[key].as_u64())
    }
    /// Bytes the snapshot copies (data + config).
    pub fn copy_bytes(&self) -> Option<u64> {
        Some(
            self.usage_u64("dataBytes")?
                .saturating_add(self.usage_u64("configBytes")?),
        )
    }
}

#[derive(Debug, Clone)]
pub enum Preflight {
    Noop(String),
    Ready(Plan),
    Blocked(Plan, Vec<Blocker>),
}

/// The fix for any helper problem: the root installer of THIS daemon's
/// version (it ships the matching helper).
fn helper_fix_hint(what: &str) -> String {
    format!("{what} — run as root: {}", helper::install_command())
}

/// Read-only preflight. Effects recorded by a fake are only reads
/// (`helper version`, `helper usage`, `systemctl? is-active`).
pub fn preflight(ops: &dyn SystemOps, rt: &Runtime, arch: &str) -> Preflight {
    let mut plan = Plan {
        from: None,
        to: STALWART_PINNED_VERSION,
        prior_status: supervisor::row_field("status"),
        was_running: false,
        target: None,
        previous: None,
        helper: None,
        usage: None,
    };
    let mut blockers = Vec::new();

    let Some(status) = plan.prior_status.clone() else {
        blockers.push(Blocker::new(
            "not_installed",
            "hosted mail is not installed on this box — nothing to upgrade \
             (k2 hostmail enable installs the pinned version directly)",
        ));
        return Preflight::Blocked(plan, blockers);
    };
    let installed = supervisor::row_field("installed_version");
    plan.from = installed.clone();
    if status == "installing" || installed.is_none() {
        blockers.push(Blocker::new(
            "enable_incomplete",
            "hosted mail enable has not finished (no installed version recorded) — \
             finish `k2 hostmail enable` first; upgrade only moves a completed install",
        ));
        return Preflight::Blocked(plan, blockers);
    }
    let installed = installed.unwrap_or_default();
    if installed == STALWART_PINNED_VERSION {
        return Preflight::Noop(installed);
    }
    if installed != STALWART_PREVIOUS_VERSION {
        blockers.push(Blocker::new(
            "unknown_version",
            format!(
                "installed Stalwart {installed} is not {STALWART_PREVIOUS_VERSION} — this daemon \
                 only upgrades {STALWART_PREVIOUS_VERSION} → {STALWART_PINNED_VERSION} (a \
                 rollback must be able to reinstall the old binary)"
            ),
        ));
    }
    if supervisor::row_field("api_url").is_none() || supervisor::row_field("api_key_ref").is_none()
    {
        blockers.push(Blocker::new(
            "enable_incomplete",
            "no management API key recorded — finish `k2 hostmail enable` first (the \
             health check needs it)",
        ));
    }
    if !ops.path_exists(STALWART_BIN) {
        blockers.push(Blocker::new(
            "binary_missing",
            format!("{STALWART_BIN} is missing — nothing to upgrade from"),
        ));
    }
    if !ops.path_exists(STALWART_CONFIG) {
        blockers.push(Blocker::new(
            "store_missing",
            format!("{STALWART_CONFIG} is missing — the store was never initialized"),
        ));
    }

    // A previous run that never finished, or whose rollback failed, may
    // have left the only good copy in the snapshot. A new snapshot would
    // replace it, so the owner must look first.
    let previous_run = load_progress();
    match previous_run.as_ref().and_then(|p| p["state"].as_str()) {
        Some("running") if !rt.upgrade_busy => blockers.push(Blocker::new(
            "upgrade_interrupted",
            format!(
                "the last upgrade never finished (the daemon stopped mid-run, at step '{}') — \
                 check `k2 hostmail status` and that Stalwart serves mail; the pre-upgrade \
                 snapshot is {STALWART_SNAPSHOT_DIR}. Then run `k2 hostmail upgrade \
                 --acknowledge-failed`",
                previous_run
                    .as_ref()
                    .and_then(|p| p["step"].as_str())
                    .unwrap_or("?")
            ),
        )),
        Some("rollback_failed") => blockers.push(Blocker::new(
            "rollback_failed",
            format!(
                "the last upgrade's rollback failed — follow its manual steps (k2 hostmail \
                 status --json → upgrade.manualSteps; the snapshot at {STALWART_SNAPSHOT_DIR} \
                 may be the only good copy), then run `k2 hostmail upgrade \
                 --acknowledge-failed`"
            ),
        )),
        _ => {}
    }
    if rt.upgrade_busy {
        blockers.push(Blocker::new(
            "upgrade_in_progress",
            supervisor::UPGRADE_RUNNING_HINT,
        ));
    } else if rt.enable_busy {
        blockers.push(Blocker::new(
            "busy",
            "a hosted-mail enable or maintenance pass is running — retry when it finishes",
        ));
    }
    if rt.imports_running > 0 {
        blockers.push(Blocker::new(
            "import_running",
            format!(
                "{} mail import(s) running — wait for them to finish (an upgrade never \
                 re-runs an import)",
                rt.imports_running
            ),
        ));
    }

    match (artifact_for_arch(arch), previous_artifact_for_arch(arch)) {
        (Ok(t), Ok(p)) => {
            plan.target = Some(t);
            plan.previous = Some(p);
        }
        (Err(e), _) | (_, Err(e)) => blockers.push(Blocker::new("unsupported_arch", e)),
    }

    // The root door: present, allowed, and built for exactly this table.
    let state = ops.mail_helper_state();
    if let Some(msg) = helper::unavailable_message(state) {
        blockers.push(Blocker::new("mail_helper_missing", msg));
    } else {
        match ops.helper_query("version") {
            Err(_) => blockers.push(Blocker::new(
                "mail_helper_outdated",
                helper_fix_hint(
                    "the mail helper on this box predates `k2 hostmail upgrade` (it has no \
                     `version` verb)",
                ),
            )),
            Ok(raw) => match serde_json::from_str::<serde_json::Value>(raw.trim()) {
                Err(_) => blockers.push(Blocker::new(
                    "mail_helper_outdated",
                    helper_fix_hint("the mail helper's `version` answer is not JSON"),
                )),
                Ok(v) => {
                    if let Some(why) = helper_mismatch(&v, plan.target, plan.previous) {
                        blockers.push(Blocker::new("mail_helper_outdated", helper_fix_hint(&why)));
                    }
                    plan.helper = Some(v);
                }
            },
        }
        match ops.helper_query("usage") {
            Err(e) => blockers.push(Blocker::new(
                "usage_failed",
                format!("could not measure the data dir through the mail helper: {e}"),
            )),
            Ok(raw) => match serde_json::from_str::<serde_json::Value>(raw.trim()) {
                Err(_) => blockers.push(Blocker::new(
                    "usage_failed",
                    "the mail helper's `usage` answer is not JSON",
                )),
                Ok(u) => {
                    if let Some(b) = space_blocker(&u) {
                        blockers.push(b);
                    }
                    plan.usage = Some(u);
                }
            },
        }
    }

    plan.was_running = ops.systemctl_query(&["is-active", STALWART_UNIT]).trim() == "active";
    if blockers.is_empty() {
        Preflight::Ready(plan)
    } else {
        Preflight::Blocked(plan, blockers)
    }
}

/// `None` when the helper's `version` JSON matches this daemon exactly.
fn helper_mismatch(
    v: &serde_json::Value,
    target: Option<&StalwartArtifact>,
    previous: Option<&StalwartArtifact>,
) -> Option<String> {
    let protocol = v["protocol"].as_u64().unwrap_or(0);
    if protocol < u64::from(UPGRADE_MIN_PROTOCOL) {
        return Some(format!(
            "the mail helper speaks protocol {protocol}; upgrade needs {UPGRADE_MIN_PROTOCOL}"
        ));
    }
    let pin = v["stalwartPin"].as_str().unwrap_or("");
    let prev = v["stalwartPrevious"].as_str().unwrap_or("");
    if pin != STALWART_PINNED_VERSION || prev != STALWART_PREVIOUS_VERSION {
        return Some(format!(
            "the mail helper was built for Stalwart {pin} (previous {prev}); this daemon \
             needs {STALWART_PINNED_VERSION} (previous {STALWART_PREVIOUS_VERSION})"
        ));
    }
    let shas_match = |key: &str, art: Option<&StalwartArtifact>| match art {
        Some(a) => v[key].as_str() == Some(a.sha256),
        None => true,
    };
    if !shas_match("stalwartPinSha256", target) || !shas_match("stalwartPreviousSha256", previous) {
        return Some("the mail helper's Stalwart checksums differ from this daemon's".to_string());
    }
    None
}

/// CAL14: free ≥ data + config + 1 GiB on the snapshot's filesystem (the
/// old snapshot is replaced, so it counts as free).
fn space_blocker(u: &serde_json::Value) -> Option<Blocker> {
    let get = |k: &str| u[k].as_u64();
    let (Some(data), Some(config), Some(snap), Some(free)) = (
        get("dataBytes"),
        get("configBytes"),
        get("snapshotBytes"),
        get("freeBytes"),
    ) else {
        return Some(Blocker::new(
            "usage_failed",
            "the mail helper's `usage` answer is missing sizes",
        ));
    };
    let need = data
        .saturating_add(config)
        .saturating_add(SNAPSHOT_FREE_MARGIN);
    let have = free.saturating_add(snap);
    if have < need {
        return Some(Blocker::new(
            "insufficient_space",
            format!(
                "not enough free space for the pre-upgrade snapshot: need {} (data {} + \
                 config {} + 1 GiB), have {} (free {} + old snapshot {}) — free space \
                 under /var/lib first",
                human_bytes(need),
                human_bytes(data),
                human_bytes(config),
                human_bytes(have),
                human_bytes(free),
                human_bytes(snap)
            ),
        ));
    }
    None
}

pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

// ── Outage estimate + the printed plan ──────────────────────────────────

const STOP_SECS: u64 = 10;
const INSTALL_SECS: u64 = 5;
const START_SECS: u64 = 15;

/// Seconds a full health check can take when it fails (ping budget + the
/// IMAP greetings).
pub fn health_budget_secs() -> u64 {
    u64::from(PING_ATTEMPTS) * PING_INTERVAL_MS / 1000
        + u64::from(GREET_ATTEMPTS) * 2 * GREET_TIMEOUT.as_secs()
}

/// Outage window: from the stop to the health check. `typicalSecs` = a
/// successful upgrade; `worstCaseSecs` = a failed health check and a full
/// rollback (restore copy + old binary + its health check).
pub fn outage_estimate(copy_bytes: u64) -> serde_json::Value {
    let copy = copy_bytes.div_ceil(COPY_BYTES_PER_SEC).max(1);
    let typical = STOP_SECS + copy + INSTALL_SECS + START_SECS;
    let worst = STOP_SECS
        + copy
        + INSTALL_SECS
        + START_SECS
        + health_budget_secs()
        + STOP_SECS
        + copy
        + INSTALL_SECS
        + START_SECS
        + health_budget_secs();
    serde_json::json!({
        "typicalSecs": typical,
        "worstCaseSecs": worst,
        "copySecs": copy,
        "assumption": format!(
            "copy at ~{} MiB/s; stop {STOP_SECS}s, install {INSTALL_SECS}s, start \
             {START_SECS}s; a failing health check waits up to {}s",
            COPY_BYTES_PER_SEC / (1024 * 1024),
            health_budget_secs()
        ),
    })
}

fn plan_steps(plan: &Plan) -> Vec<String> {
    let copy = plan
        .copy_bytes()
        .map(human_bytes)
        .unwrap_or_else(|| "size unknown".into());
    let mut steps = vec![
        format!(
            "download + sha256-verify Stalwart {STALWART_PINNED_VERSION} and \
             {STALWART_PREVIOUS_VERSION} (Stalwart keeps serving)"
        ),
        "stop Stalwart (mail helper: systemctl stop stalwart) — the outage starts".to_string(),
        format!(
            "snapshot {} + {} → {STALWART_SNAPSHOT_DIR} ({copy}; replaces any older snapshot)",
            supervisor::STALWART_DATA_DIR,
            supervisor::STALWART_CONFIG_DIR
        ),
        format!("install Stalwart {STALWART_PINNED_VERSION} → {STALWART_BIN} (atomic rename)"),
        "start Stalwart (mail helper: systemctl restart stalwart)".to_string(),
        "health check: unit active + JMAP ping on the loopback management API + IMAP \
         greeting on the stored IMAP listener"
            .to_string(),
    ];
    if !plan.was_running {
        steps.push(format!(
            "stop Stalwart again — it was '{}' before the upgrade and stays that way",
            plan.prior_status.as_deref().unwrap_or("?")
        ));
    }
    steps
}

fn rollback_steps(plan: &Plan) -> Vec<String> {
    let mut steps = vec![
        "a failure before the snapshot changes nothing (the old binary is started again if \
         it was running)"
            .to_string(),
        "after the snapshot: stop Stalwart".to_string(),
        format!("restore {STALWART_SNAPSHOT_DIR} over the data + config dirs (mail helper)"),
        format!("reinstall Stalwart {STALWART_PREVIOUS_VERSION}"),
    ];
    if plan.was_running {
        steps.push("start Stalwart + the same health check".to_string());
    }
    steps.push(
        "if any rollback step fails: state rollback_failed, hostmail status error, exact \
         manual steps in `k2 hostmail status --json`"
            .to_string(),
    );
    steps
}

/// The `--dry-run` body. Never has side effects.
pub fn dry_run_json(plan: &Plan, blockers: &[Blocker]) -> serde_json::Value {
    let copy = plan.copy_bytes();
    serde_json::json!({
        "ok": true,
        "dryRun": true,
        "ready": blockers.is_empty(),
        "blockers": blockers.iter().map(Blocker::json).collect::<Vec<_>>(),
        "from": plan.from,
        "to": plan.to,
        "state": plan.prior_status,
        "wasRunning": plan.was_running,
        "helper": plan.helper,
        "dataBytes": plan.usage_u64("dataBytes"),
        "configBytes": plan.usage_u64("configBytes"),
        "freeBytes": plan.usage_u64("freeBytes"),
        "snapshotDir": STALWART_SNAPSHOT_DIR,
        "outage": copy.map(outage_estimate),
        "plan": plan_steps(plan),
        "rollback": rollback_steps(plan),
        "notes": [
            "never automatic: a daemon update or enable never upgrades Stalwart",
            "never re-runs a Maildir import; mail not yet imported (e.g. a cPanel tail) is \
             not in the snapshot",
            "never uses hostmail disable/enable",
        ],
    })
}

// ── Progress (mail_server.upgrade_progress_json) ────────────────────────

pub fn load_progress() -> Option<serde_json::Value> {
    supervisor::row_field(PROGRESS_COL).and_then(|s| serde_json::from_str(&s).ok())
}

fn save_progress(v: &serde_json::Value) {
    supervisor::set_row_field(PROGRESS_COL, &v.to_string());
}

/// `upgrade` for `/cli/mail/status`: the stored record, with a `running`
/// record that no thread owns reported as `interrupted` (the daemon died
/// mid-run) plus the same manual steps a failed rollback gets.
pub fn status_upgrade_json(upgrade_busy: bool) -> serde_json::Value {
    let Some(mut p) = load_progress() else {
        return serde_json::Value::Null;
    };
    if p["state"] == "running" && !upgrade_busy {
        p["state"] = serde_json::json!("interrupted");
        if p.get("manualSteps").is_none() {
            p["manualSteps"] = serde_json::json!(interrupted_steps(&p));
        }
    }
    p
}

/// Manual steps for a run the daemon never finished, by how far it got.
/// Before the install the data and the binary are untouched, so the fix
/// is only "start it again" — restoring then could bring back an OLDER
/// snapshot and lose mail.
pub fn interrupted_steps(p: &serde_json::Value) -> Vec<String> {
    let was_running = p["wasRunning"].as_bool().unwrap_or(true);
    let step = p["step"].as_str().unwrap_or("");
    let untouched =
        step.is_empty() || step.starts_with("download") || step == "stop" || step == "snapshot";
    if untouched {
        return untouched_steps(if step.is_empty() { "start" } else { step }, was_running);
    }
    let mut steps = vec![format!(
        "the run stopped at '{step}' — after the snapshot. Check {STALWART_SNAPSHOT_DIR}/\
         {STALWART_SNAPSHOT_MARKER} createdAt >= the upgrade's startedAt ({}) before restoring",
        p["startedAt"]
    )];
    steps.extend(manual_steps(
        previous_artifact_for_arch(std::env::consts::ARCH).ok(),
        was_running,
    ));
    steps
}

/// Manual steps when the data and the binary were never touched (a stop
/// or snapshot failure, or a run interrupted before the install): only
/// start the old binary again. Never a restore.
pub fn untouched_steps(step: &str, was_running: bool) -> Vec<String> {
    let mut steps = vec![format!(
        "the run stopped at '{step}' — before the install: data and binary are unchanged \
         (do NOT restore {STALWART_SNAPSHOT_DIR}; it may be older)"
    )];
    if was_running {
        steps.push("sudo systemctl start stalwart && k2 hostmail status".into());
    } else {
        steps.push("k2 hostmail status   # it was stopped before the upgrade".into());
    }
    steps.push("k2 hostmail upgrade --acknowledge-failed".into());
    steps
}

/// `upgradeAvailable`: installed and not the pin (CAL15/CAL56: `!=`).
pub fn upgrade_available(installed: Option<&str>) -> bool {
    installed.is_some_and(|v| v != STALWART_PINNED_VERSION)
}

/// The probe the health check uses (live: JMAP + a TCP/TLS IMAP greeting).
pub trait UpgradeProbe {
    fn ping(&self) -> Result<(), String>;
    /// The greeting line, or a note when no IMAP listener is stored.
    fn imap_greet(&self) -> Result<String, String>;
}

/// One upgrade run: the effects, the persisted record, the outcome.
struct Run<'a> {
    ops: &'a dyn SystemOps,
    doors: &'a mut dyn RestartDoors,
    probe: &'a dyn UpgradeProbe,
    plan: &'a Plan,
    progress: serde_json::Value,
}

impl Run<'_> {
    fn step(&mut self, step: &str) {
        self.progress["step"] = serde_json::json!(step);
        save_progress(&self.progress);
    }

    fn done(&mut self, step: &str, result: &Result<String, String>) {
        let (ok, detail) = match result {
            Ok(d) => (true, d.clone()),
            Err(e) => (false, e.clone()),
        };
        let entry = serde_json::json!({
            "step": step,
            "at": supervisor::now_secs(),
            "ok": ok,
            "detail": detail,
        });
        if let Some(list) = self.progress["steps"].as_array_mut() {
            list.push(entry);
        }
        save_progress(&self.progress);
    }

    /// Run one step: mark it current, run it, record the result.
    fn exec(
        &mut self,
        step: &str,
        f: impl FnOnce(&mut Self) -> Result<String, String>,
    ) -> Result<String, String> {
        self.step(step);
        let r = f(self);
        self.done(step, &r);
        r
    }

    fn finish(&mut self, state: &str, error: Option<String>) -> serde_json::Value {
        self.progress["state"] = serde_json::json!(state);
        self.progress["step"] = serde_json::Value::Null;
        self.progress["finishedAt"] = serde_json::json!(supervisor::now_secs());
        if let Some(e) = error {
            self.progress["error"] = serde_json::json!(e);
        }
        save_progress(&self.progress);
        self.progress.clone()
    }

    fn stop(&mut self) -> Result<String, String> {
        self.ops.systemctl(&["stop", STALWART_UNIT])?;
        wait_stopped(self.ops)
    }

    /// Start through the IMAP-listener restart doors (helper first, then
    /// plain sudo): the one restart path, reused.
    fn start(&mut self) -> Result<String, String> {
        match imap_listeners::restart_once(self.doors)? {
            RestartReport::Restarted(path) => Ok(format!("started ({path:?} door)")),
            RestartReport::NotRestarted { helper } => Err(format!(
                "no door can start Stalwart (mail helper: {})",
                helper.as_str()
            )),
        }
    }

    fn health(&mut self) -> Result<String, String> {
        health_check(self.ops, self.probe)
    }

    fn install(&mut self, bytes: &[u8]) -> Result<String, String> {
        self.ops
            .extract_tar_gz_member(bytes, "stalwart", STALWART_BIN, 0o755)
            .map(|()| format!("installed {STALWART_BIN}"))
    }
}

/// After `systemctl stop`: bounded wait for `inactive`/`failed`.
fn wait_stopped(ops: &dyn SystemOps) -> Result<String, String> {
    let mut last = String::new();
    for _ in 0..50 {
        last = ops.systemctl_query(&["is-active", STALWART_UNIT]);
        if matches!(last.trim(), "inactive" | "failed") {
            return Ok(format!("stopped ({})", last.trim()));
        }
        ops.sleep_ms(200);
    }
    Err(format!(
        "stalwart did not stop (systemd: '{}')",
        if last.trim().is_empty() {
            "unknown"
        } else {
            last.trim()
        }
    ))
}

/// Unit active, then the JMAP ping (retried while the store opens), then
/// the IMAP greeting.
pub fn health_check(ops: &dyn SystemOps, probe: &dyn UpgradeProbe) -> Result<String, String> {
    let mut last = String::from("no attempt");
    let mut pinged = false;
    for attempt in 0..PING_ATTEMPTS {
        if attempt > 0 {
            ops.sleep_ms(PING_INTERVAL_MS);
        }
        let unit = ops.systemctl_query(&["is-active", STALWART_UNIT]);
        if unit.trim() != "active" {
            last = format!("systemd reports the stalwart unit is '{}'", unit.trim());
            if matches!(unit.trim(), "failed" | "inactive") {
                break; // crashed or exited: no point waiting
            }
            continue;
        }
        match probe.ping() {
            Ok(()) => {
                pinged = true;
                break;
            }
            Err(e) => last = format!("JMAP ping failed: {e}"),
        }
    }
    if !pinged {
        return Err(format!("health check failed: {last}"));
    }
    let mut greet_err = String::new();
    for attempt in 0..GREET_ATTEMPTS {
        if attempt > 0 {
            ops.sleep_ms(PING_INTERVAL_MS);
        }
        match probe.imap_greet() {
            Ok(line) => return Ok(format!("JMAP ping ok; IMAP: {line}")),
            Err(e) => greet_err = e,
        }
    }
    Err(format!(
        "health check failed: JMAP ping ok but the IMAP greeting failed: {greet_err}"
    ))
}

/// Manual repair when the automatic rollback failed (arch-specific sha).
pub fn manual_steps(previous: Option<&StalwartArtifact>, was_running: bool) -> Vec<String> {
    let snap = STALWART_SNAPSHOT_DIR;
    let data = supervisor::STALWART_DATA_DIR;
    let config = supervisor::STALWART_CONFIG_DIR;
    let mut steps = vec![
        "sudo systemctl stop stalwart".to_string(),
        format!(
            "sudo ls {snap}/{STALWART_SNAPSHOT_MARKER}   # must exist: the snapshot is complete"
        ),
        format!("sudo mv {data} {data}.k2-failed-$(date +%s) && sudo cp -a {snap}/data {data}"),
        format!(
            "sudo mv {config} {config}.k2-failed-$(date +%s) && sudo cp -a {snap}/config {config}"
        ),
    ];
    if let Some(p) = previous {
        let tgz = format!("/tmp/stalwart-{STALWART_PREVIOUS_VERSION}.tar.gz");
        steps.push(format!(
            "curl -fsSLo {tgz} {}",
            tarball_url_for(STALWART_PREVIOUS_VERSION, p.triple)
        ));
        steps.push(format!("echo '{}  {tgz}' | sha256sum -c -", p.sha256));
        steps.push(format!(
            "sudo tar --no-same-owner -xzf {tgz} -C /usr/local/bin stalwart && sudo chown \
             root:root {STALWART_BIN} && sudo chmod 0755 {STALWART_BIN}"
        ));
    }
    if was_running {
        steps.push("sudo systemctl start stalwart && k2 hostmail status".to_string());
    } else {
        steps.push("k2 hostmail status   # Stalwart stays stopped, as before the upgrade".into());
    }
    steps.push("k2 hostmail upgrade --acknowledge-failed".to_string());
    steps
}

/// Run the upgrade on an already-preflighted plan. Returns the final
/// record (also persisted). The caller holds the enable latch and the
/// upgrade flag.
pub fn run(
    ops: &dyn SystemOps,
    doors: &mut dyn RestartDoors,
    probe: &dyn UpgradeProbe,
    plan: &Plan,
) -> serde_json::Value {
    let mut r = Run {
        ops,
        doors,
        probe,
        plan,
        progress: initial_progress(plan),
    };
    save_progress(&r.progress);
    let (Some(target), Some(previous)) = (plan.target, plan.previous) else {
        return r.finish("aborted", Some("no artifact for this arch".into()));
    };
    let was_running = plan.was_running;

    // 1. Downloads (both, before the stop): Stalwart keeps serving, a
    //    failure changes nothing, and the rollback never needs the network.
    let target_bytes = match fetch_verified(&mut r, STALWART_PINNED_VERSION, target) {
        Ok(b) => b,
        Err(e) => return r.finish("aborted", Some(format!("download: {e} — nothing changed"))),
    };
    let previous_bytes = match fetch_verified(&mut r, STALWART_PREVIOUS_VERSION, previous) {
        Ok(b) => b,
        Err(e) => return r.finish("aborted", Some(format!("download: {e} — nothing changed"))),
    };

    // 2. Stop. 3. Snapshot. A failure here has changed no data and no binary.
    let pre_change = r.exec("stop", |r| r.stop()).and_then(|_| {
        r.exec("snapshot", |r| {
            r.ops.snapshot_data().map(|_| "snapshot complete".into())
        })
    });
    if let Err(e) = pre_change {
        return abort_unchanged(&mut r, e);
    }
    r.progress["snapshotDir"] = serde_json::json!(STALWART_SNAPSHOT_DIR);

    // 4. Install. 5. Start. 6. Health.
    let upgraded = r
        .exec("install", |r| r.install(&target_bytes))
        .and_then(|_| r.exec("start", |r| r.start()))
        .and_then(|_| r.exec("health", |r| r.health()));
    if let Err(e) = upgraded {
        return rollback(&mut r, e, &previous_bytes);
    }

    // Success.
    supervisor::set_row_field("installed_version", STALWART_PINNED_VERSION);
    supervisor::set_row_field("pinned_version", STALWART_PINNED_VERSION);
    let mut warning = None;
    if !was_running {
        if let Err(e) = r.exec("stop-again", |r| r.stop()) {
            warning = Some(format!(
                "upgraded, but Stalwart could not be stopped again ({e}) — it was '{}' \
                 before the upgrade and is running now; run `k2 hostmail disable` if it \
                 should stay off",
                plan.prior_status.as_deref().unwrap_or("?")
            ));
        }
    }
    if let Some(w) = &warning {
        r.progress["warning"] = serde_json::json!(w);
    }
    r.finish("succeeded", None)
}

fn initial_progress(plan: &Plan) -> serde_json::Value {
    serde_json::json!({
        "state": "running",
        "from": plan.from,
        "to": STALWART_PINNED_VERSION,
        "priorStatus": plan.prior_status,
        "wasRunning": plan.was_running,
        "startedAt": supervisor::now_secs(),
        "step": "download",
        "steps": [],
        "snapshotDir": serde_json::Value::Null,
    })
}

/// Download `version`'s tarball once and verify it against the compiled
/// sha256. The verified bytes are what gets installed.
fn fetch_verified(r: &mut Run, version: &str, art: &StalwartArtifact) -> Result<Vec<u8>, String> {
    let url = tarball_url_for(version, art.triple);
    let mut kept = None;
    r.exec(&format!("download-{version}"), |r| {
        let bytes = r.ops.download(&url)?;
        if !crate::update_routes::verify_sha256(&bytes, art.sha256) {
            return Err(format!(
                "sha256 mismatch for {url} — not installing (expected {})",
                art.sha256
            ));
        }
        let note = format!("{} bytes, sha256 ok", bytes.len());
        kept = Some(bytes);
        Ok(note)
    })?;
    kept.ok_or_else(|| "download produced no bytes".to_string())
}

/// Failure at stop / snapshot: no data or binary changed. Put the old
/// binary back in service if it was serving.
fn abort_unchanged(r: &mut Run, err: String) -> serde_json::Value {
    if r.plan.was_running {
        let back = r
            .exec("restart-old", |r| r.start())
            .and_then(|_| r.exec("restart-old-health", |r| r.health()));
        if let Err(e) = back {
            // Nothing changed, so the repair is a start — never a restore
            // (the snapshot dir may hold an OLDER snapshot).
            let steps = untouched_steps("restart-old", true);
            return rollback_failed(
                r,
                format!(
                    "{err}; nothing was changed, but Stalwart {} could not be started again: {e}",
                    r.plan.from.as_deref().unwrap_or("?")
                ),
                steps,
            );
        }
    }
    r.finish(
        "aborted",
        Some(format!("{err} — nothing was changed (no data, no binary)")),
    )
}

/// Failure after the snapshot: stop → restore → reinstall previous →
/// start + health (if it was running).
fn rollback(r: &mut Run, err: String, previous_bytes: &[u8]) -> serde_json::Value {
    let back = r
        .exec("rollback-stop", |r| r.stop())
        .and_then(|_| {
            r.exec("rollback-restore", |r| {
                r.ops.restore_data().map(|_| "snapshot restored".into())
            })
        })
        .and_then(|_| r.exec("rollback-install", |r| r.install(previous_bytes)));
    let back = match back {
        Ok(_) if r.plan.was_running => r
            .exec("rollback-start", |r| r.start())
            .and_then(|_| r.exec("rollback-health", |r| r.health())),
        other => other,
    };
    if let Err(e) = back {
        // This run's snapshot completed (rollback only follows it), so the
        // restore steps point at the right copy.
        let steps = manual_steps(r.plan.previous, r.plan.was_running);
        return rollback_failed(r, format!("{err}; ROLLBACK FAILED: {e}"), steps);
    }
    r.finish(
        "rolled_back",
        Some(format!(
            "{err} — rolled back: snapshot restored, Stalwart {STALWART_PREVIOUS_VERSION} \
             reinstalled{}",
            if r.plan.was_running {
                " and serving again"
            } else {
                " (left stopped, as before)"
            }
        )),
    )
}

fn rollback_failed(r: &mut Run, msg: String, steps: Vec<String>) -> serde_json::Value {
    r.progress["manualSteps"] = serde_json::json!(steps);
    let previous = r
        .plan
        .prior_status
        .clone()
        .unwrap_or_else(|| "unknown".into());
    let status_msg = format!(
        "upgrade to {STALWART_PINNED_VERSION} failed and its rollback failed: {msg} — manual \
         steps: k2 hostmail status --json (upgrade.manualSteps)"
    );
    supervisor::set_status("error");
    supervisor::set_last_error(Some(&status_msg));
    supervisor::emit_state_change(&previous, "error", Some(&status_msg));
    r.finish("rollback_failed", Some(msg))
}

/// `--acknowledge-failed`: clear an interrupted / rollback_failed block
/// after the manual repair. Upgrades nothing.
pub fn acknowledge(upgrade_busy: bool) -> Result<serde_json::Value, Blocker> {
    if upgrade_busy {
        return Err(Blocker::new(
            "upgrade_in_progress",
            supervisor::UPGRADE_RUNNING_HINT,
        ));
    }
    let Some(mut p) = load_progress() else {
        return Err(Blocker::new(
            "nothing_to_acknowledge",
            "no upgrade has run on this box",
        ));
    };
    let state = p["state"].as_str().unwrap_or("").to_string();
    if state != "rollback_failed" && state != "running" {
        return Err(Blocker::new(
            "nothing_to_acknowledge",
            format!("the last upgrade ended '{state}' — nothing to acknowledge"),
        ));
    }
    p["acknowledgedState"] = serde_json::json!(if state == "running" {
        "interrupted"
    } else {
        state.as_str()
    });
    p["state"] = serde_json::json!("acknowledged");
    p["acknowledgedAt"] = serde_json::json!(supervisor::now_secs());
    save_progress(&p);
    Ok(p)
}

// ── Live wiring ──────────────────────────────────────────────────────────

/// Releases the latches when the background run ends (even by panic).
struct LatchGuard;

impl Drop for LatchGuard {
    fn drop(&mut self) {
        supervisor::end_enable();
        supervisor::upgrade_running().store(false, Ordering::SeqCst);
    }
}

fn refused(b: &Blocker, all: &[Blocker], plan: Option<&Plan>) -> UpgradeStart {
    UpgradeStart::Refused {
        code: b.code,
        hint: b.hint.clone(),
        body: serde_json::json!({
            "ok": false,
            "error": { "code": b.code, "hint": b.hint },
            "blockers": all.iter().map(Blocker::json).collect::<Vec<_>>(),
            "from": plan.and_then(|p| p.from.clone()),
            "to": STALWART_PINNED_VERSION,
        }),
    }
}

fn noop_json(version: &str) -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "outcome": "noop",
        "version": version,
        "pinnedVersion": STALWART_PINNED_VERSION,
        "hint": format!("Stalwart {version} is already the pinned version — nothing to do"),
    })
}

/// Preflight → dry-run plan, refusal, no-op, or start the run on a
/// background thread. Live entry for [`supervisor::upgrade`].
pub fn start_live(req: UpgradeRequest) -> UpgradeStart {
    if req.acknowledge_failed {
        return match acknowledge(supervisor::upgrade_running().load(Ordering::SeqCst)) {
            Ok(p) => UpgradeStart::Acknowledged(serde_json::json!({
                "ok": true,
                "outcome": "acknowledged",
                "upgrade": p,
                "hint": "acknowledged — next: k2 hostmail upgrade --dry-run",
            })),
            Err(b) => refused(&b, std::slice::from_ref(&b), None),
        };
    }
    let ops = super::sysops::RealSystemOps;
    let plan = match preflight(&ops, &Runtime::live(), std::env::consts::ARCH) {
        Preflight::Noop(v) => return UpgradeStart::Noop(noop_json(&v)),
        Preflight::Blocked(plan, blockers) => {
            if req.dry_run {
                return UpgradeStart::DryRun(dry_run_json(&plan, &blockers));
            }
            return refused(&blockers[0], &blockers, Some(&plan));
        }
        Preflight::Ready(plan) => plan,
    };
    if req.dry_run {
        return UpgradeStart::DryRun(dry_run_json(&plan, &[]));
    }
    // Claim: upgrade flag first, then the import count (imports increment
    // first, then read the flag — so the two never both proceed), then
    // the shared enable latch (health loop, DAV backfill, IMAP reconcile,
    // enable all stay out).
    if supervisor::upgrade_running()
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        let b = Blocker::new("upgrade_in_progress", supervisor::UPGRADE_RUNNING_HINT);
        return refused(&b, std::slice::from_ref(&b), Some(&plan));
    }
    if super::import::imports_running() > 0 {
        supervisor::upgrade_running().store(false, Ordering::SeqCst);
        let b = Blocker::new(
            "import_running",
            "a mail import started — retry when it finishes",
        );
        return refused(&b, std::slice::from_ref(&b), Some(&plan));
    }
    if !supervisor::try_begin_enable() {
        supervisor::upgrade_running().store(false, Ordering::SeqCst);
        let b = Blocker::new(
            "busy",
            "a hosted-mail enable or maintenance pass is running — retry when it finishes",
        );
        return refused(&b, std::slice::from_ref(&b), Some(&plan));
    }
    save_progress(&initial_progress(&plan));
    let body = serde_json::json!({
        "ok": true,
        "started": true,
        "from": plan.from,
        "to": STALWART_PINNED_VERSION,
        "wasRunning": plan.was_running,
        "outage": plan.copy_bytes().map(outage_estimate),
        "hint": "upgrading in the background — poll GET /cli/mail/status (upgrade)",
    });
    let spawned = std::thread::Builder::new()
        .name("mail-upgrade".into())
        .spawn(move || {
            let _latch = LatchGuard;
            let ops = super::sysops::RealSystemOps;
            let mut doors = imap_listeners::LiveDoors { why: "upgrade" };
            let probe = LiveProbe;
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(&ops, &mut doors, &probe, &plan)
            }));
            match outcome {
                Ok(v) => k2_core::log_debug!(
                    "[mail/upgrade] finished: {}",
                    v["state"].as_str().unwrap_or("?")
                ),
                // The record stays `running` → reported `interrupted`, and
                // the next upgrade is blocked until acknowledged.
                Err(_) => k2_core::log_debug!("[mail/upgrade] PANICKED mid-run"),
            }
        });
    if let Err(e) = spawned {
        supervisor::end_enable();
        supervisor::upgrade_running().store(false, Ordering::SeqCst);
        let b = Blocker::new(
            "spawn_failed",
            format!("could not start the upgrade thread: {e}"),
        );
        save_progress(&serde_json::json!({ "state": "aborted", "error": b.hint }));
        return refused(&b, std::slice::from_ref(&b), None);
    }
    UpgradeStart::Started(body)
}

/// Production probe: the loopback management API + the stored IMAP
/// listener.
pub struct LiveProbe;

impl UpgradeProbe for LiveProbe {
    fn ping(&self) -> Result<(), String> {
        supervisor::mgmt_client_from_row()?.ping()
    }

    fn imap_greet(&self) -> Result<String, String> {
        let rows = supervisor::mgmt_client_from_row()?.listener_rows()?;
        let imap: Vec<_> = rows
            .iter()
            .filter(|r| r.protocol.as_deref() == Some("imap"))
            .collect();
        let Some(pick) = imap
            .iter()
            .find(|r| r.tls_implicit == Some(false))
            .or_else(|| imap.first())
        else {
            return Ok("no IMAP listener stored — greeting skipped".into());
        };
        let implicit = pick.tls_implicit == Some(true);
        let mut last = format!("listener '{}' has no bind", pick.name);
        for bind in &pick.binds {
            for addr in connect_targets(bind) {
                match greet(&addr, implicit) {
                    Ok(line) => return Ok(format!("{} {addr}: {line}", pick.name)),
                    Err(e) => last = format!("{addr}: {e}"),
                }
            }
        }
        Err(last)
    }
}

/// Loopback addresses for a stored bind: a wildcard bind is reached on
/// 127.0.0.1 and ::1; a specific address on itself.
pub fn connect_targets(bind: &str) -> Vec<String> {
    let bind = bind.trim();
    let Some((host, port)) = bind.rsplit_once(':') else {
        return Vec::new();
    };
    if port.parse::<u16>().is_err() {
        return Vec::new();
    }
    match host {
        "[::]" | "0.0.0.0" | "*" | "" => {
            vec![format!("127.0.0.1:{port}"), format!("[::1]:{port}")]
        }
        _ => vec![format!("{host}:{port}")],
    }
}

/// `* OK` / `* PREAUTH` greeting check over one already-read line.
pub fn greeting_ok(line: &str) -> Result<String, String> {
    let t = line.trim();
    if t.starts_with("* OK") || t.starts_with("* PREAUTH") {
        Ok(t.chars().take(120).collect())
    } else if t.is_empty() {
        Err("no IMAP greeting".into())
    } else {
        Err(format!(
            "unexpected IMAP greeting: {}",
            t.chars().take(120).collect::<String>()
        ))
    }
}

fn read_line(stream: &mut dyn std::io::Read) -> Result<String, String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while buf.len() < 512 {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                buf.push(byte[0]);
                if byte[0] == b'\n' {
                    break;
                }
            }
            Err(e) => return Err(format!("read: {e}")),
        }
    }
    Ok(String::from_utf8_lossy(&buf).to_string())
}

fn greet(addr: &str, implicit_tls: bool) -> Result<String, String> {
    use std::net::{TcpStream, ToSocketAddrs};
    let sock_addr = addr
        .to_socket_addrs()
        .map_err(|e| format!("resolve: {e}"))?
        .next()
        .ok_or("no address")?;
    let mut sock = TcpStream::connect_timeout(&sock_addr, GREET_TIMEOUT)
        .map_err(|e| format!("connect: {e}"))?;
    sock.set_read_timeout(Some(GREET_TIMEOUT))
        .map_err(|e| format!("timeout: {e}"))?;
    sock.set_write_timeout(Some(GREET_TIMEOUT))
        .map_err(|e| format!("timeout: {e}"))?;
    if !implicit_tls {
        return greeting_ok(&read_line(&mut sock)?);
    }
    let config = loopback_tls_config()?;
    let name = rustls::pki_types::ServerName::try_from("localhost".to_string())
        .map_err(|e| format!("tls name: {e}"))?;
    let mut conn = rustls::ClientConnection::new(config, name).map_err(|e| format!("tls: {e}"))?;
    let mut tls = rustls::Stream::new(&mut conn, &mut sock);
    greeting_ok(&read_line(&mut tls)?)
}

/// TLS for the loopback greeting only: the cert is for the public mail
/// hostname, not 127.0.0.1, and this checks liveness, not identity. No
/// credential is ever sent on this connection.
fn loopback_tls_config() -> Result<std::sync::Arc<rustls::ClientConfig>, String> {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, SignatureScheme};

    #[derive(Debug)]
    struct LivenessOnly;
    impl ServerCertVerifier for LivenessOnly {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            rustls::crypto::aws_lc_rs::default_provider()
                .signature_verification_algorithms
                .supported_schemes()
        }
    }
    let builder = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| format!("tls: {e}"))?;
    Ok(std::sync::Arc::new(
        builder
            .dangerous()
            .with_custom_certificate_verifier(std::sync::Arc::new(LivenessOnly))
            .with_no_client_auth(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::helper::HelperState;
    use crate::mail::sysops::fake::FakeSystemOps;
    use sha2::{Digest, Sha256};
    use std::collections::VecDeque;
    use std::sync::Mutex;

    const ARCH: &str = "x86_64";
    const TARGET_TGZ: &[u8] = b"fake stalwart 0.16.20 tarball";
    const PREV_TGZ: &[u8] = b"fake stalwart 0.16.10 tarball";
    const EXTRACT: &str = "extract stalwart -> /usr/local/bin/stalwart (mode 755)";
    const GIB: u64 = 1 << 30;

    fn hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn fake_art(bytes: &[u8]) -> &'static StalwartArtifact {
        Box::leak(Box::new(StalwartArtifact {
            arch: ARCH,
            triple: "x86_64-unknown-linux-gnu",
            sha256: Box::leak(hex(bytes).into_boxed_str()),
        }))
    }

    fn target_url() -> String {
        tarball_url_for(STALWART_PINNED_VERSION, "x86_64-unknown-linux-gnu")
    }
    fn prev_url() -> String {
        tarball_url_for(STALWART_PREVIOUS_VERSION, "x86_64-unknown-linux-gnu")
    }

    fn seed_row(status: &str, installed: Option<&str>) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("DELETE FROM mail_server WHERE id = 1", [])
            .expect("clear row");
        conn.execute(
            "INSERT INTO mail_server (id, status, pinned_version, installed_version, hostname, \
             port_plan, api_url, api_key_ref, updated_at) VALUES (1, ?1, ?2, ?3, \
             'mail.example.com', 'tls-alpn', 'http://127.0.0.1:8180', 'mail-secret:api-key', 100)",
            rusqlite::params![status, STALWART_PREVIOUS_VERSION, installed],
        )
        .expect("seed mail_server");
    }

    fn clean_row() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("DELETE FROM mail_server WHERE id = 1", [])
            .expect("clear row");
    }

    fn usage_json(data: u64, config: u64, snap: u64, free: u64) -> String {
        serde_json::json!({
            "dataBytes": data, "configBytes": config,
            "snapshotBytes": snap, "snapshotComplete": snap > 0, "freeBytes": free,
        })
        .to_string()
    }

    /// A box on the previous pin, helper current, running, plenty of room.
    fn ready_ops() -> FakeSystemOps {
        let mut ops = FakeSystemOps {
            existing_paths: vec![STALWART_BIN.into(), STALWART_CONFIG.into()],
            ..FakeSystemOps::default()
        };
        ops.helper_answers
            .insert("version".into(), helper::version_json_for(ARCH).to_string());
        ops.helper_answers
            .insert("usage".into(), usage_json(3 * GIB, 4096, 0, 50 * GIB));
        ops.download_bodies
            .insert(target_url(), TARGET_TGZ.to_vec());
        ops.download_bodies.insert(prev_url(), PREV_TGZ.to_vec());
        ops.set_unit_active(Some(true));
        ops
    }

    fn idle() -> Runtime {
        Runtime::default()
    }

    struct FakeProbe<'a> {
        ops: &'a FakeSystemOps,
        pings: Mutex<VecDeque<Result<(), String>>>,
        greets: Mutex<VecDeque<Result<String, String>>>,
    }

    impl<'a> FakeProbe<'a> {
        fn new(ops: &'a FakeSystemOps) -> Self {
            Self {
                ops,
                pings: Mutex::new(VecDeque::new()),
                greets: Mutex::new(VecDeque::new()),
            }
        }
        fn fail_pings(&self, n: u32) {
            let mut q = self.pings.lock().unwrap();
            for _ in 0..n {
                q.push_back(Err("connection refused".into()));
            }
        }
        fn fail_greets(&self, n: u32) {
            let mut q = self.greets.lock().unwrap();
            for _ in 0..n {
                q.push_back(Err("connect: refused".into()));
            }
        }
    }

    impl UpgradeProbe for FakeProbe<'_> {
        fn ping(&self) -> Result<(), String> {
            self.ops.record("ping".into());
            self.pings.lock().unwrap().pop_front().unwrap_or(Ok(()))
        }
        fn imap_greet(&self) -> Result<String, String> {
            self.ops.record("imap-greet".into());
            self.greets
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok("* OK Stalwart IMAP4rev2 ready".into()))
        }
    }

    /// The helper door of the IMAP-listener restart path, over the fake
    /// (production passes `imap_listeners::LiveDoors`, whose helper arm is
    /// `restart_stalwart_and_wait` — the same call).
    struct OpsDoors<'a>(&'a FakeSystemOps);

    impl RestartDoors for OpsDoors<'_> {
        fn helper_state(&mut self) -> HelperState {
            self.0.mail_helper_state()
        }
        fn plain_sudo_allowed(&mut self) -> bool {
            self.0.record("plain-sudo?".into());
            false
        }
        fn restart(&mut self, _path: imap_listeners::RestartPath) -> Result<(), String> {
            supervisor::restart_stalwart_and_wait_with(self.0, "upgrade")
        }
    }

    /// Effects + probe calls, without the read-only `systemctl?` queries.
    fn effects(ops: &FakeSystemOps) -> Vec<String> {
        ops.recorded()
            .into_iter()
            .filter(|l| !l.starts_with("systemctl? "))
            .collect()
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn downloads() -> Vec<String> {
        vec![
            format!("download {}", target_url()),
            format!("download {}", prev_url()),
        ]
    }

    fn concat(parts: &[Vec<String>]) -> Vec<String> {
        parts.iter().flatten().cloned().collect()
    }

    /// Preflight against the seeded row, then point the plan at the fake
    /// tarballs (the run verifies the downloads against these shas).
    fn ready_plan(ops: &FakeSystemOps) -> Plan {
        let plan = match preflight(ops, &idle(), ARCH) {
            Preflight::Ready(p) => p,
            other => panic!("expected Ready, got {other:?}"),
        };
        Plan {
            target: Some(fake_art(TARGET_TGZ)),
            previous: Some(fake_art(PREV_TGZ)),
            ..plan
        }
    }

    fn run_with(ops: &FakeSystemOps, probe: &FakeProbe, plan: &Plan) -> serde_json::Value {
        ops.ops.lock().unwrap().clear();
        let mut doors = OpsDoors(ops);
        run(ops, &mut doors, probe, plan)
    }

    fn only_reads(ops: &FakeSystemOps) {
        for line in ops.recorded() {
            assert!(
                line.starts_with("systemctl? ")
                    || line == "helper version"
                    || line == "helper usage",
                "preflight / dry-run must not have effects, got {line:?} in {:?}",
                ops.recorded()
            );
        }
    }

    fn blocked_codes(p: Preflight) -> Vec<&'static str> {
        match p {
            Preflight::Blocked(_, b) => b.iter().map(|x| x.code).collect(),
            other => panic!("expected Blocked, got {other:?}"),
        }
    }

    // ── preflight ─────────────────────────────────────────────────────

    #[test]
    fn preflight_ready_on_previous_pin_reads_only() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row("running", Some(STALWART_PREVIOUS_VERSION));
        let ops = ready_ops();
        let p = match preflight(&ops, &idle(), ARCH) {
            Preflight::Ready(p) => p,
            other => panic!("expected Ready, got {other:?}"),
        };
        assert_eq!(p.from.as_deref(), Some(STALWART_PREVIOUS_VERSION));
        assert_eq!(p.to, STALWART_PINNED_VERSION);
        assert!(p.was_running);
        assert_eq!(p.copy_bytes(), Some(3 * GIB + 4096));
        only_reads(&ops);
        clean_row();
    }

    #[test]
    fn preflight_noop_when_installed_is_the_pin_and_calls_nothing() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row("running", Some(STALWART_PINNED_VERSION));
        let ops = ready_ops();
        match preflight(&ops, &idle(), ARCH) {
            Preflight::Noop(v) => assert_eq!(v, STALWART_PINNED_VERSION),
            other => panic!("expected Noop, got {other:?}"),
        }
        assert!(ops.recorded().is_empty(), "{:?}", ops.recorded());
        clean_row();
    }

    #[test]
    fn preflight_refuses_when_not_installed_or_enable_unfinished() {
        let _g = crate::mail::mail_server_test_lock();
        clean_row();
        let ops = ready_ops();
        assert_eq!(
            blocked_codes(preflight(&ops, &idle(), ARCH)),
            vec!["not_installed"]
        );
        seed_row("installing", None);
        assert_eq!(
            blocked_codes(preflight(&ops, &idle(), ARCH)),
            vec!["enable_incomplete"]
        );
        seed_row("error", None);
        assert_eq!(
            blocked_codes(preflight(&ops, &idle(), ARCH)),
            vec!["enable_incomplete"]
        );
        assert!(ops.recorded().is_empty(), "{:?}", ops.recorded());
        clean_row();
    }

    #[test]
    fn preflight_refuses_an_unknown_installed_version() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row("running", Some("0.17.0"));
        let ops = ready_ops();
        let Preflight::Blocked(_, b) = preflight(&ops, &idle(), ARCH) else {
            panic!("expected Blocked");
        };
        assert_eq!(b[0].code, "unknown_version");
        assert!(b[0].hint.contains("0.17.0"), "{}", b[0].hint);
        only_reads(&ops);
        clean_row();
    }

    #[test]
    fn preflight_refuses_missing_binary_store_or_api_key() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row("running", Some(STALWART_PREVIOUS_VERSION));
        let mut ops = ready_ops();
        ops.existing_paths.clear();
        let codes = blocked_codes(preflight(&ops, &idle(), ARCH));
        assert!(codes.contains(&"binary_missing"), "{codes:?}");
        assert!(codes.contains(&"store_missing"), "{codes:?}");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("UPDATE mail_server SET api_key_ref = NULL WHERE id = 1", [])
                .expect("null key");
        }
        let codes = blocked_codes(preflight(&ops, &idle(), ARCH));
        assert!(codes.contains(&"enable_incomplete"), "{codes:?}");
        only_reads(&ops);
        clean_row();
    }

    /// CAL40 (a box with no helper at all): refuse with the root install
    /// command, before any helper query.
    #[test]
    fn preflight_refuses_without_the_helper_with_the_install_command() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row("running", Some(STALWART_PREVIOUS_VERSION));
        for state in [HelperState::Missing, HelperState::NotAllowed] {
            let ops = FakeSystemOps {
                helper_state: state,
                ..ready_ops()
            };
            let Preflight::Blocked(_, b) = preflight(&ops, &idle(), ARCH) else {
                panic!("expected Blocked");
            };
            assert_eq!(b.len(), 1, "{b:?}");
            assert_eq!(b[0].code, "mail_helper_missing");
            assert!(
                b[0].hint.contains(&helper::install_command()),
                "{}",
                b[0].hint
            );
            assert!(
                !ops.recorded().iter().any(|l| l.starts_with("helper ")),
                "no helper query without a helper: {:?}",
                ops.recorded()
            );
        }
        clean_row();
    }

    #[test]
    fn preflight_refuses_an_older_or_mismatched_helper() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row("running", Some(STALWART_PREVIOUS_VERSION));
        // Protocol 1: no `version` verb ("refused arguments").
        let mut ops = ready_ops();
        ops.helper_answers.remove("version");
        let Preflight::Blocked(_, b) = preflight(&ops, &idle(), ARCH) else {
            panic!("expected Blocked");
        };
        assert_eq!(b[0].code, "mail_helper_outdated");
        assert!(b[0].hint.contains("predates"), "{}", b[0].hint);
        assert!(
            b[0].hint.ends_with(&helper::install_command()),
            "{}",
            b[0].hint
        );

        let mut variants = Vec::new();
        let mut v = helper::version_json_for(ARCH);
        v["protocol"] = serde_json::json!(1);
        variants.push(("protocol 1", v));
        let mut v = helper::version_json_for(ARCH);
        v["stalwartPin"] = serde_json::json!("0.16.25");
        variants.push(("other pin", v));
        let mut v = helper::version_json_for(ARCH);
        v["stalwartPreviousSha256"] = serde_json::json!("00".repeat(32));
        variants.push(("other sha", v));
        variants.push(("not json", serde_json::json!("protocol 2")));
        for (what, v) in variants {
            let mut ops = ready_ops();
            let raw = if what == "not json" {
                "protocol 2".to_string()
            } else {
                v.to_string()
            };
            ops.helper_answers.insert("version".into(), raw);
            let Preflight::Blocked(_, b) = preflight(&ops, &idle(), ARCH) else {
                panic!("{what}: expected Blocked");
            };
            assert_eq!(b[0].code, "mail_helper_outdated", "{what}");
            assert!(
                b[0].hint.contains(&helper::install_command()),
                "{what}: {}",
                b[0].hint
            );
            only_reads(&ops);
        }
        clean_row();
    }

    #[test]
    fn preflight_refuses_without_room_for_the_snapshot() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row("running", Some(STALWART_PREVIOUS_VERSION));
        let mut ops = ready_ops();
        // data 10 GiB, free 10 GiB: short by the 1 GiB margin.
        ops.helper_answers
            .insert("usage".into(), usage_json(10 * GIB, 0, 0, 10 * GIB));
        let Preflight::Blocked(_, b) = preflight(&ops, &idle(), ARCH) else {
            panic!("expected Blocked");
        };
        assert_eq!(b[0].code, "insufficient_space");
        assert!(b[0].hint.contains("11.0 GiB"), "{}", b[0].hint);
        // The old snapshot is replaced, so it counts as free.
        ops.helper_answers
            .insert("usage".into(), usage_json(10 * GIB, 0, 2 * GIB, 10 * GIB));
        assert!(matches!(
            preflight(&ops, &idle(), ARCH),
            Preflight::Ready(_)
        ));
        // A usage answer without sizes, or a failed query, is a refusal.
        ops.helper_answers.insert("usage".into(), "{}".into());
        assert_eq!(
            blocked_codes(preflight(&ops, &idle(), ARCH)),
            vec!["usage_failed"]
        );
        ops.helper_answers.remove("usage");
        assert_eq!(
            blocked_codes(preflight(&ops, &idle(), ARCH)),
            vec!["usage_failed"]
        );
        only_reads(&ops);
        clean_row();
    }

    #[test]
    fn preflight_refuses_while_an_import_enable_or_upgrade_runs() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row("running", Some(STALWART_PREVIOUS_VERSION));
        let ops = ready_ops();
        let rt = Runtime {
            imports_running: 2,
            ..Runtime::default()
        };
        assert_eq!(
            blocked_codes(preflight(&ops, &rt, ARCH)),
            vec!["import_running"]
        );
        let rt = Runtime {
            enable_busy: true,
            ..Runtime::default()
        };
        assert_eq!(blocked_codes(preflight(&ops, &rt, ARCH)), vec!["busy"]);
        let rt = Runtime {
            enable_busy: true,
            upgrade_busy: true,
            ..Runtime::default()
        };
        assert_eq!(
            blocked_codes(preflight(&ops, &rt, ARCH)),
            vec!["upgrade_in_progress"]
        );
        only_reads(&ops);
        clean_row();
    }

    #[test]
    fn preflight_blocks_after_an_interrupted_or_failed_run_until_acknowledged() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row("error", Some(STALWART_PREVIOUS_VERSION));
        let ops = ready_ops();
        save_progress(&serde_json::json!({"state": "running", "step": "snapshot"}));
        let Preflight::Blocked(_, b) = preflight(&ops, &idle(), ARCH) else {
            panic!("expected Blocked");
        };
        assert_eq!(b[0].code, "upgrade_interrupted");
        assert!(b[0].hint.contains("snapshot"), "{}", b[0].hint);
        let interrupted = status_upgrade_json(false);
        assert_eq!(interrupted["state"], "interrupted");
        let early = interrupted["manualSteps"].to_string();
        assert!(early.contains("do NOT restore"), "before install: {early}");
        assert!(
            !early.contains("cp -a"),
            "never restore an older snapshot: {early}"
        );
        assert_eq!(status_upgrade_json(true)["state"], "running");
        assert!(status_upgrade_json(true).get("manualSteps").is_none());
        // Interrupted after the install: the restore steps, with the
        // snapshot-age check first.
        let late = interrupted_steps(&serde_json::json!({
            "state": "running", "step": "health", "wasRunning": true, "startedAt": 1234
        }))
        .join("\n");
        assert!(
            late.contains("createdAt") && late.contains("1234"),
            "{late}"
        );
        assert!(
            late.contains(&format!("{STALWART_SNAPSHOT_DIR}/data")),
            "{late}"
        );
        assert!(late.contains("sudo systemctl start stalwart"), "{late}");

        save_progress(&serde_json::json!({"state": "rollback_failed"}));
        assert_eq!(
            blocked_codes(preflight(&ops, &idle(), ARCH)),
            vec!["rollback_failed"]
        );
        assert_eq!(
            acknowledge(true).expect_err("busy").code,
            "upgrade_in_progress"
        );
        let acked = acknowledge(false).expect("acknowledge");
        assert_eq!(acked["state"], "acknowledged");
        assert_eq!(acked["acknowledgedState"], "rollback_failed");
        assert!(matches!(
            preflight(&ops, &idle(), ARCH),
            Preflight::Ready(_)
        ));
        let again = acknowledge(false).expect_err("nothing left to acknowledge");
        assert_eq!(again.code, "nothing_to_acknowledge");
        clean_row();
        assert_eq!(
            acknowledge(false).expect_err("no row").code,
            "nothing_to_acknowledge"
        );
    }

    #[test]
    fn dry_run_prints_the_plan_outage_and_rollback_with_no_effects() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row("running", Some(STALWART_PREVIOUS_VERSION));
        let ops = ready_ops();
        let Preflight::Ready(plan) = preflight(&ops, &idle(), ARCH) else {
            panic!("expected Ready");
        };
        let d = dry_run_json(&plan, &[]);
        assert_eq!(d["dryRun"], true);
        assert_eq!(d["ready"], true);
        assert_eq!(d["from"], STALWART_PREVIOUS_VERSION);
        assert_eq!(d["to"], STALWART_PINNED_VERSION);
        assert_eq!(d["dataBytes"], 3 * GIB);
        assert_eq!(d["snapshotDir"], STALWART_SNAPSHOT_DIR);
        let typical = d["outage"]["typicalSecs"].as_u64().expect("typical");
        let worst = d["outage"]["worstCaseSecs"].as_u64().expect("worst");
        // 3 GiB + 4 KiB at 100 MiB/s → 31 s copy; + stop 10, install 5, start 15.
        assert_eq!(d["outage"]["copySecs"], 31);
        assert_eq!(typical, 61);
        assert!(worst > 2 * typical, "worst {worst} typical {typical}");
        let plan_text = d["plan"].to_string();
        for want in [
            "stop Stalwart",
            "snapshot",
            "install Stalwart",
            "health check",
        ] {
            assert!(
                plan_text.contains(want),
                "plan must say {want}: {plan_text}"
            );
        }
        let rb = d["rollback"].to_string();
        assert!(
            rb.contains("restore") && rb.contains(STALWART_PREVIOUS_VERSION),
            "{rb}"
        );
        assert!(d["notes"]
            .to_string()
            .contains("never re-runs a Maildir import"));
        only_reads(&ops);
        assert!(load_progress().is_none(), "dry-run must not write progress");

        // A blocked dry-run still answers, with ready=false + blockers.
        let blocked = dry_run_json(&plan, &[Blocker::new("import_running", "1 import")]);
        assert_eq!(blocked["ready"], false);
        assert_eq!(blocked["blockers"][0]["code"], "import_running");
        clean_row();
    }

    // ── the run ───────────────────────────────────────────────────────

    #[test]
    fn happy_path_runs_stop_snapshot_install_start_health_in_order() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row("running", Some(STALWART_PREVIOUS_VERSION));
        let ops = ready_ops();
        let plan = ready_plan(&ops);
        let probe = FakeProbe::new(&ops);
        let out = run_with(&ops, &probe, &plan);
        assert_eq!(out["state"], "succeeded", "{out}");
        assert_eq!(
            effects(&ops),
            concat(&[
                downloads(),
                s(&[
                    "systemctl stop stalwart",
                    "snapshot-data",
                    EXTRACT,
                    "systemctl restart stalwart",
                    "ping",
                    "imap-greet",
                ]),
            ])
        );
        assert_eq!(*ops.extracted.lock().unwrap(), vec![TARGET_TGZ.to_vec()]);
        assert_eq!(
            supervisor::row_field("installed_version").as_deref(),
            Some(STALWART_PINNED_VERSION)
        );
        assert_eq!(
            supervisor::row_field("pinned_version").as_deref(),
            Some(STALWART_PINNED_VERSION)
        );
        assert_eq!(supervisor::row_field("status").as_deref(), Some("running"));
        assert_eq!(out["snapshotDir"], STALWART_SNAPSHOT_DIR);
        assert_eq!(ops.unit_is_active(), Some(true));
        // Persisted for /cli/mail/status, and idempotent afterwards.
        assert_eq!(status_upgrade_json(false)["state"], "succeeded");
        assert!(matches!(preflight(&ops, &idle(), ARCH), Preflight::Noop(_)));
        // Every privileged line is a helper vector (sudo -n helper …).
        for line in ops.recorded() {
            if helper::is_privileged_recording(&line) && !line.starts_with("systemctl?") {
                helper::recorded_line_allowlisted(&line).unwrap_or_else(|e| panic!("{line}: {e}"));
            }
        }
        clean_row();
    }

    /// CAL15: works from a disabled box — started for the health check,
    /// then stopped again; the row keeps its status.
    #[test]
    fn upgrade_from_a_disabled_box_leaves_it_stopped() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row("disabled", Some(STALWART_PREVIOUS_VERSION));
        let ops = ready_ops();
        ops.set_unit_active(Some(false));
        let plan = ready_plan(&ops);
        assert!(!plan.was_running);
        let probe = FakeProbe::new(&ops);
        let out = run_with(&ops, &probe, &plan);
        assert_eq!(out["state"], "succeeded", "{out}");
        assert_eq!(
            effects(&ops),
            concat(&[
                downloads(),
                s(&[
                    "systemctl stop stalwart",
                    "snapshot-data",
                    EXTRACT,
                    "systemctl restart stalwart",
                    "ping",
                    "imap-greet",
                    "systemctl stop stalwart",
                ]),
            ])
        );
        assert_eq!(ops.unit_is_active(), Some(false));
        assert_eq!(supervisor::row_field("status").as_deref(), Some("disabled"));
        assert_eq!(
            supervisor::row_field("installed_version").as_deref(),
            Some(STALWART_PINNED_VERSION)
        );
        clean_row();
    }

    #[test]
    fn download_or_checksum_failure_aborts_before_the_stop() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row("running", Some(STALWART_PREVIOUS_VERSION));
        let ops = FakeSystemOps {
            download_error: Some("HTTP 503".into()),
            ..ready_ops()
        };
        let plan = ready_plan(&ops);
        let out = run_with(&ops, &FakeProbe::new(&ops), &plan);
        assert_eq!(out["state"], "aborted", "{out}");
        assert!(
            out["error"].as_str().unwrap().contains("nothing changed"),
            "{out}"
        );
        assert_eq!(effects(&ops), vec![format!("download {}", target_url())]);

        let mut ops = ready_ops();
        ops.download_bodies.insert(prev_url(), b"tampered".to_vec());
        let plan = ready_plan(&ops);
        let out = run_with(&ops, &FakeProbe::new(&ops), &plan);
        assert_eq!(out["state"], "aborted", "{out}");
        assert!(
            out["error"].as_str().unwrap().contains("sha256 mismatch"),
            "{out}"
        );
        assert_eq!(
            effects(&ops),
            downloads(),
            "no stop, no snapshot, no install"
        );
        assert_eq!(
            supervisor::row_field("installed_version").as_deref(),
            Some(STALWART_PREVIOUS_VERSION)
        );
        clean_row();
    }

    #[test]
    fn stop_or_snapshot_failure_restarts_the_old_binary_without_restore() {
        let _g = crate::mail::mail_server_test_lock();
        for failing in ["systemctl stop stalwart", "snapshot-data"] {
            seed_row("running", Some(STALWART_PREVIOUS_VERSION));
            let ops = ready_ops();
            let plan = ready_plan(&ops);
            ops.fail(failing);
            let out = run_with(&ops, &FakeProbe::new(&ops), &plan);
            assert_eq!(out["state"], "aborted", "{failing}: {out}");
            assert!(
                out["error"]
                    .as_str()
                    .unwrap()
                    .contains("nothing was changed"),
                "{failing}: {out}"
            );
            let mut want = downloads();
            want.push("systemctl stop stalwart".into());
            if failing == "snapshot-data" {
                want.push("snapshot-data".into());
            }
            want.extend(s(&["systemctl restart stalwart", "ping", "imap-greet"]));
            assert_eq!(effects(&ops), want, "{failing}");
            assert!(ops.extracted.lock().unwrap().is_empty(), "{failing}");
            assert_eq!(
                supervisor::row_field("installed_version").as_deref(),
                Some(STALWART_PREVIOUS_VERSION)
            );
            assert_eq!(ops.unit_is_active(), Some(true), "{failing}");
        }
        clean_row();
    }

    /// Failure at install / start / ping / IMAP greeting → stop → restore
    /// → reinstall previous → start → health: `rolled_back`.
    #[test]
    fn failure_after_the_snapshot_rolls_back_in_order() {
        let _g = crate::mail::mail_server_test_lock();
        let rollback = s(&[
            "systemctl stop stalwart",
            "restore-data",
            EXTRACT,
            "systemctl restart stalwart",
            "ping",
            "imap-greet",
        ]);
        for case in ["install", "start", "ping", "imap"] {
            seed_row("running", Some(STALWART_PREVIOUS_VERSION));
            let ops = ready_ops();
            let plan = ready_plan(&ops);
            let probe = FakeProbe::new(&ops);
            let mut head = concat(&[
                downloads(),
                s(&["systemctl stop stalwart", "snapshot-data", EXTRACT]),
            ]);
            match case {
                "install" => ops.fail(EXTRACT),
                "start" => {
                    ops.fail("systemctl restart stalwart");
                    head.push("systemctl restart stalwart".into());
                }
                "ping" => {
                    probe.fail_pings(PING_ATTEMPTS);
                    head.push("systemctl restart stalwart".into());
                    head.extend(std::iter::repeat_n(
                        "ping".to_string(),
                        PING_ATTEMPTS as usize,
                    ));
                }
                _ => {
                    probe.fail_greets(GREET_ATTEMPTS);
                    head.push("systemctl restart stalwart".into());
                    head.push("ping".into());
                    head.extend(std::iter::repeat_n(
                        "imap-greet".to_string(),
                        GREET_ATTEMPTS as usize,
                    ));
                }
            }
            let out = run_with(&ops, &probe, &plan);
            assert_eq!(out["state"], "rolled_back", "{case}: {out}");
            assert!(
                out["error"].as_str().unwrap().contains("rolled back"),
                "{case}: {out}"
            );
            assert_eq!(effects(&ops), concat(&[head, rollback.clone()]), "{case}");
            assert_eq!(
                *ops.extracted.lock().unwrap(),
                vec![TARGET_TGZ.to_vec(), PREV_TGZ.to_vec()],
                "{case}: new then old binary"
            );
            assert_eq!(
                supervisor::row_field("installed_version").as_deref(),
                Some(STALWART_PREVIOUS_VERSION),
                "{case}"
            );
            assert_eq!(
                supervisor::row_field("status").as_deref(),
                Some("running"),
                "{case}"
            );
            assert_eq!(ops.unit_is_active(), Some(true), "{case}");
            // A rolled-back run does not block the next attempt.
            assert!(
                matches!(preflight(&ops, &idle(), ARCH), Preflight::Ready(_)),
                "{case}"
            );
        }
        clean_row();
    }

    #[test]
    fn rollback_of_a_stopped_box_does_not_start_it() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row("stopped", Some(STALWART_PREVIOUS_VERSION));
        let ops = ready_ops();
        ops.set_unit_active(Some(false));
        let plan = ready_plan(&ops);
        let probe = FakeProbe::new(&ops);
        probe.fail_pings(PING_ATTEMPTS);
        let out = run_with(&ops, &probe, &plan);
        assert_eq!(out["state"], "rolled_back", "{out}");
        assert!(
            out["error"].as_str().unwrap().contains("left stopped"),
            "{out}"
        );
        let tail: Vec<String> = effects(&ops).into_iter().rev().take(3).collect();
        assert_eq!(
            tail,
            s(&[EXTRACT, "restore-data", "systemctl stop stalwart"])
        );
        assert_eq!(ops.unit_is_active(), Some(false));
        clean_row();
    }

    #[test]
    fn rollback_failure_is_loud_sets_error_and_blocks_the_next_run() {
        let _g = crate::mail::mail_server_test_lock();
        for failing in ["restore-data", "rollback-install", "rollback-health"] {
            seed_row("running", Some(STALWART_PREVIOUS_VERSION));
            let ops = ready_ops();
            let plan = ready_plan(&ops);
            let probe = FakeProbe::new(&ops);
            ops.fail(EXTRACT); // the upgrade's install fails …
            match failing {
                "restore-data" => ops.fail("restore-data"),
                "rollback-install" => ops.fail(EXTRACT), // … and the old one too
                _ => probe.fail_pings(PING_ATTEMPTS),
            }
            let out = run_with(&ops, &probe, &plan);
            assert_eq!(out["state"], "rollback_failed", "{failing}: {out}");
            assert!(
                out["error"].as_str().unwrap().contains("ROLLBACK FAILED"),
                "{failing}: {out}"
            );
            let all = out["manualSteps"]
                .as_array()
                .expect("manual steps")
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(all.contains("systemctl stop stalwart"), "{all}");
            assert!(
                all.contains(&format!("{STALWART_SNAPSHOT_DIR}/data")),
                "{all}"
            );
            assert!(all.contains(fake_art(PREV_TGZ).sha256), "{all}");
            assert!(all.contains("--acknowledge-failed"), "{all}");
            assert!(!all.contains("hostmail disable"), "{all}");
            assert_eq!(supervisor::row_field("status").as_deref(), Some("error"));
            let last = supervisor::row_field("last_error").unwrap_or_default();
            assert!(last.contains("rollback failed"), "{last}");
            assert_eq!(
                blocked_codes(preflight(&ops, &idle(), ARCH))[0],
                "rollback_failed",
                "{failing}"
            );
        }
        clean_row();
    }

    /// A snapshot failure changes nothing; if the old binary then will not
    /// start, it is loud — but the manual steps never restore (the
    /// snapshot dir may hold an OLDER snapshot).
    #[test]
    fn abort_whose_restart_fails_is_loud_without_restore_steps() {
        let _g = crate::mail::mail_server_test_lock();
        seed_row("running", Some(STALWART_PREVIOUS_VERSION));
        let ops = ready_ops();
        let plan = ready_plan(&ops);
        ops.fail("snapshot-data");
        ops.fail("systemctl restart stalwart");
        let out = run_with(&ops, &FakeProbe::new(&ops), &plan);
        assert_eq!(out["state"], "rollback_failed", "{out}");
        let steps = out["manualSteps"].to_string();
        assert!(steps.contains("do NOT restore"), "{steps}");
        assert!(!steps.contains("cp -a"), "{steps}");
        assert!(steps.contains("sudo systemctl start stalwart"), "{steps}");
        assert!(!effects(&ops)
            .iter()
            .any(|l| l == "restore-data" || l == EXTRACT));
        assert_eq!(supervisor::row_field("status").as_deref(), Some("error"));
        clean_row();
    }

    #[test]
    fn health_check_stops_waiting_when_the_unit_died() {
        let ops = FakeSystemOps::default();
        ops.set_unit_active(Some(false));
        let probe = FakeProbe::new(&ops);
        let err = health_check(&ops, &probe).expect_err("dead unit");
        assert!(err.contains("inactive"), "{err}");
        assert!(
            !ops.recorded().iter().any(|l| l == "ping"),
            "{:?}",
            ops.recorded()
        );
    }

    #[test]
    fn upgrade_available_is_inequality_not_ordering() {
        assert!(upgrade_available(Some(STALWART_PREVIOUS_VERSION)));
        assert!(upgrade_available(Some("0.17.0")), "!= — never semver");
        assert!(!upgrade_available(Some(STALWART_PINNED_VERSION)));
        assert!(!upgrade_available(None));
    }

    #[test]
    fn imap_greeting_and_targets_parse() {
        assert_eq!(
            connect_targets("[::]:143"),
            s(&["127.0.0.1:143", "[::1]:143"])
        );
        assert_eq!(
            connect_targets("0.0.0.0:993"),
            s(&["127.0.0.1:993", "[::1]:993"])
        );
        assert_eq!(connect_targets("192.0.2.5:143"), s(&["192.0.2.5:143"]));
        assert!(connect_targets("nonsense").is_empty());
        assert!(greeting_ok("* OK [CAPABILITY IMAP4rev2] ready\r\n").is_ok());
        assert!(greeting_ok("* PREAUTH hi\r\n").is_ok());
        assert!(greeting_ok("").is_err());
        assert!(greeting_ok("HTTP/1.1 400 Bad Request").is_err());
    }

    #[test]
    fn outage_estimate_has_a_floor_and_scales() {
        let small = outage_estimate(0);
        assert_eq!(small["copySecs"], 1);
        let big = outage_estimate(100 * GIB);
        assert_eq!(big["copySecs"], 1024);
        assert!(big["worstCaseSecs"].as_u64() > big["typicalSecs"].as_u64());
    }
}
