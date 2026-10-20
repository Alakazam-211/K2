//! Automatic App box prep (0.46.3, Rosson 2026-10-20: "I should be able to
//! turn on a permissions toggle to let him use apps/runtimes"): a Linux box
//! with the owner's root helper gets ready for walled Apps by itself after
//! an update — K2's Podman bundle where the distro's Podman is < 5, the
//! k2app user and its sub-ids, delegation, linger, the nft table, the
//! AppArmor profile (exactly `box_prep::plan`, the owner's `k2 app box
//! prepare`), then the App engine (`engine_install`) — so an agent with
//! "Build and run Apps" can install and deploy with nobody running sudo.
//!
//! **Consent.** The owner installed the root helper (that is what lets K2
//! run root steps at all; the helper's own allowlist bounds them) and the
//! owner-only setting `appsAutoPrepare` is on (`K2_APPS_AUTO_PREPARE`
//! overrides; default `deploy::AUTO_PREPARE_DEFAULT`, OFF until the design
//! `design-apps-auto-prepare-0463` is signed off). Box prep never needed a
//! vault confirm (it is not on the AR43 person-present list: `may_prepare`
//! is an owner-caller check, and the daemon itself is the caller here).
//!
//! **Never breaks the box.** Before any change it reads `/etc/subuid` and
//! `/etc/subgid` (world-readable) and stops on any overlap between owners
//! (rootless Docker under the daemon user, a hand-made range): the doctor
//! and the owner's note say which. A box that is already ready is left
//! alone (no root call). Below 5 GiB free on `/` or `/var` nothing starts
//! (F5). The box's own Podman is never upgraded or doubled (F4, in the
//! helper). The whole run holds the shared root latch (`root_helper::busy`,
//! F9), so it never interleaves with a hosted-mail upgrade or another root
//! sequence. macOS and other OSes: nothing.
//!
//! **When.** One thread from boot (= after every update), ~3 minutes in
//! (after the root helper's own heal), then daily; a failure retries after
//! 1 h, doubling to 24 h; a held latch retries in 5 minutes. A box that
//! can't be prepared (kernel < 5.11, cgroup v1, old systemd, a distro with
//! no Podman 5 and no K2 bundle, a hand-made sub-id overlap, a source-built
//! Podman K2 must not shadow) is **blocked** (F6): recorded, shown, and not
//! tried again until the owner presses Retry (`POST
//! /cli/app/box/auto-prepare/retry`) or K2 updates. The owner's "Undo box
//! setup" (`box_unprep`) holds it the same way until Retry.
//!
//! **Status** (F7) lives in `~/.k2/apps-auto-prepare.json` (last run,
//! outcome, what changed, next attempt, the K2 version, an undo hold), so
//! `GET /cli/app/box` → `autoPrepare` and Settings → Apps → This server are
//! right straight after a restart. A run that changed the box files one
//! owner notice (what was installed, how to undo it).

use std::path::PathBuf;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

use super::routes::{need_credential, no, R};
use crate::compute_routes::Who;
use k2_core::connect_users::Role;

/// The helper protocol the whole prep needs (K2's Podman bundle install is
/// protocol 8; the rest is 6).
pub const MIN_PROTOCOL: u32 = k2_root_proto::version::PODMAN_INSTALL_MIN_PROTOCOL;
/// Who the audit and the notice name.
pub const ACTOR: &str = "k2:auto-prepare";
/// The latch holder's name in refusals ("… is running").
pub const LATCH_NAME: &str = "automatic App box prep";
/// The owner's Retry.
pub const RETRY_ROUTE: &str = "/cli/app/box/auto-prepare/retry";

/// What one run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Not run this time: `code` (`not_linux`, `apps_off`, `setting_off`,
    /// `no_helper`, `helper_old`, `airgap`, `low_disk`, `busy`) and why.
    Skipped { code: String, why: String },
    /// The box was ready already: nothing changed.
    Ready,
    /// Prepared: what changed, in order.
    Prepared { changed: Vec<String> },
    /// A step failed; the box is where the doctor says. Retried with backoff.
    Failed { step: String, error: String },
    /// The box can't be prepared as it is (F6): no retry until the owner
    /// presses Retry or K2 updates. `code`: `kernel_too_old`, `cgroup_v1`,
    /// `systemd_too_old`, `distro_unsupported`, `cpu_unsupported`,
    /// `customer_podman`, `subid_overlap`, `apt_would_remove`,
    /// `policy_missing`, `owner_undid`.
    Blocked { code: String, why: String },
}

fn skip(code: &str, why: impl Into<String>) -> Outcome {
    Outcome::Skipped { code: code.into(), why: why.into() }
}

fn blocked(code: &str, why: impl Into<String>) -> Outcome {
    Outcome::Blocked { code: code.into(), why: why.into() }
}

impl Outcome {
    pub fn json(&self) -> J {
        match self {
            Outcome::Skipped { code, why } => json!({ "state": "skipped", "code": code, "why": why }),
            Outcome::Ready => json!({ "state": "ready" }),
            Outcome::Prepared { changed } => json!({ "state": "prepared", "changed": changed }),
            Outcome::Failed { step, error } => json!({ "state": "failed", "step": step, "error": error }),
            Outcome::Blocked { code, why } => json!({ "state": "blocked", "code": code, "why": why }),
        }
    }

    /// Back from [`Outcome::json`] (the status file).
    pub fn from_json(v: &J) -> Option<Self> {
        let s = |k: &str| v[k].as_str().map(str::to_string);
        Some(match v["state"].as_str()? {
            "skipped" => Outcome::Skipped { code: s("code")?, why: s("why")? },
            "ready" => Outcome::Ready,
            "prepared" => Outcome::Prepared {
                changed: v["changed"].as_array()?.iter().filter_map(|c| c.as_str().map(str::to_string)).collect(),
            },
            "failed" => Outcome::Failed { step: s("step")?, error: s("error")? },
            "blocked" => Outcome::Blocked { code: s("code")?, why: s("why")? },
            _ => return None,
        })
    }

    pub fn is_blocked(&self) -> bool {
        matches!(self, Outcome::Blocked { .. })
    }
}

/// The box as the run sees it (tests drive a fake).
pub trait Ops {
    fn linux(&self) -> bool;
    fn apps_on(&self) -> bool;
    fn enabled(&self) -> bool;
    /// The owner ran "Undo box setup" and hasn't pressed Retry since.
    fn undone(&self) -> bool;
    /// `None` = no root helper installed (or not root-owned); else its
    /// protocol.
    fn helper_protocol(&self) -> Option<u32>;
    /// `GET /cli/app/box`'s doctor (`ready`, `checks`).
    fn doctor(&self) -> J;
    /// `/etc/subuid`, `/etc/subgid` (empty when missing).
    fn subid_files(&self) -> (String, String);
    fn airgap(&self) -> bool;
    /// Bytes free per checked path (`k2_root_proto::apps::FREE_CHECK_PATHS`).
    fn disk_free(&self) -> Vec<(String, u64)>;
    /// Claim the shared root latch (`root_helper::busy`); `false` = held.
    fn begin_busy(&self) -> bool;
    fn end_busy(&self);
    /// The owner's prepare, with K2's Podman when the box needs it.
    fn prepare(&self) -> Result<J, String>;
    fn install_engine(&self) -> Result<J, String>;
    fn note_owner(&self, title: &str, body: &str);
}

// ── sub-id overlaps ──────────────────────────────────────────────────

fn parse_subid(text: &str) -> Vec<(String, u64, u64)> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.trim().split(':').collect();
            if f.len() != 3 || f[0].starts_with('#') {
                return None;
            }
            let start: u64 = f[1].trim().parse().ok()?;
            let count: u64 = f[2].trim().parse().ok()?;
            (count > 0).then(|| (f[0].trim().to_string(), start, count))
        })
        .collect()
}

/// Two owners' ranges that overlap in one file, named (`None` = none).
/// The prep's own `apps subid` allocates above every range; an overlap
/// that is already there (rootless Docker's range across someone else's)
/// means the box's sub-ids are hand-made and K2 stays out.
pub fn subid_conflict(subuid: &str, subgid: &str) -> Option<String> {
    for (file, text) in [("/etc/subuid", subuid), ("/etc/subgid", subgid)] {
        let rows = parse_subid(text);
        for (i, a) in rows.iter().enumerate() {
            for b in rows.iter().skip(i + 1) {
                if a.0 != b.0 && a.1 < b.1 + b.2 && b.1 < a.1 + a.2 {
                    return Some(format!(
                        "{file}: {}'s range {}+{} overlaps {}'s {}+{}",
                        a.0, a.1, a.2, b.0, b.1, b.2
                    ));
                }
            }
        }
    }
    None
}

// ── permanent problems (F6) ──────────────────────────────────────────

fn row<'a>(doctor: &'a J, id: &str) -> Option<&'a J> {
    doctor["checks"].as_array()?.iter().find(|c| c["id"] == id)
}

/// A doctor row that says the box itself can't run walled Apps, whatever
/// prep does: `(code, why)`.
pub fn permanent_from_doctor(doctor: &J) -> Option<(&'static str, String)> {
    let fail = |id: &str| row(doctor, id).filter(|c| c["status"] == "fail");
    let detail = |c: &J| c["detail"].as_str().unwrap_or("").to_string();
    if let Some(c) = fail("kernel") {
        return Some(("kernel_too_old", detail(c)));
    }
    if let Some(c) = fail("cgroupV2") {
        return Some(("cgroup_v1", format!("{}: walled Apps need the unified cgroup v2 hierarchy", detail(c))));
    }
    if let Some(c) = fail("systemd") {
        return Some(("systemd_too_old", detail(c)));
    }
    // Podman < 5 with no fix: the distro has no Podman 5 and K2's bundle
    // isn't for this OS (the doctor offers a fix whenever prep has one).
    if let Some(c) = fail("podman").filter(|c| c["fix"].is_null()) {
        return Some((
            "distro_unsupported",
            format!("{}; K2's own Podman covers Ubuntu 24.04 and Debian 12 only", detail(c)),
        ));
    }
    None
}

/// Root-helper and route refusals that won't change by trying again
/// (first match wins: `podman_os_unsupported` before `unsupported: no`).
const PERMANENT_ERRORS: &[(&str, &str)] = &[
    ("podman_too_old", "distro_unsupported"),
    ("podman_os_unsupported", "distro_unsupported"),
    ("no supported package manager", "distro_unsupported"),
    ("customer_podman", "customer_podman"),
    ("apt_would_remove", "apt_would_remove"),
    ("policy_missing", "policy_missing"),
    ("unsupported: no ", "cpu_unsupported"),
];

pub fn permanent_from_error(e: &str) -> Option<&'static str> {
    PERMANENT_ERRORS.iter().find(|(needle, _)| e.contains(needle)).map(|(_, code)| *code)
}

// ── one run ──────────────────────────────────────────────────────────

fn needs(doctor: &J) -> (bool, bool) {
    let checks = doctor["checks"].as_array().cloned().unwrap_or_default();
    let not_ok = |c: &J| c["status"].as_str() != Some("ok");
    let prepare = checks
        .iter()
        .any(|c| not_ok(c) && c["fix"].as_str().is_some_and(|f| f.starts_with("k2 app box prepare")));
    let engine = checks.iter().any(|c| c["id"] == "engine" && not_ok(c));
    (prepare, engine)
}

/// Releases the latch however the steps end (a panic included).
struct Release<'a>(&'a dyn Ops);

impl Drop for Release<'_> {
    fn drop(&mut self) {
        self.0.end_busy();
    }
}

/// One run: decide, then prepare and install the engine where needed.
pub fn run_once(ops: &dyn Ops) -> Outcome {
    if !ops.linux() {
        return skip("not_linux", "walled Apps run on Linux boxes only");
    }
    if !ops.apps_on() {
        return skip("apps_off", "Apps (beta) are off on this server");
    }
    if !ops.enabled() {
        return skip("setting_off", "the owner's \"Prepare this box automatically\" (appsAutoPrepare) is off");
    }
    if ops.undone() {
        return blocked(
            "owner_undid",
            "the owner undid the box setup (Undo box setup / k2 app box unprep); automatic prep waits until the owner presses Retry",
        );
    }
    match ops.helper_protocol() {
        None => {
            return skip(
                "no_helper",
                "K2's root helper isn't installed: the owner installs it once (Settings → K2 Server), or prepares the box by hand",
            )
        }
        Some(p) if p < MIN_PROTOCOL => {
            return skip(
                "helper_old",
                format!("the root helper speaks protocol {p}; automatic prep needs {MIN_PROTOCOL} (it updates itself after a K2 update)"),
            )
        }
        Some(_) => {}
    }
    let doctor = ops.doctor();
    // Before `needs`: a kernel or cgroup row has no prep fix, so a box that
    // can never run Apps would otherwise read as "nothing to prepare".
    if let Some((code, why)) = permanent_from_doctor(&doctor) {
        return blocked(code, format!("{why}. Prep can't fix this box; K2 won't try again until you press Retry or K2 updates"));
    }
    let (prepare, engine) = needs(&doctor);
    if !prepare && !engine {
        return Outcome::Ready;
    }
    let (subuid, subgid) = ops.subid_files();
    if let Some(c) = subid_conflict(&subuid, &subgid) {
        return blocked(
            "subid_overlap",
            format!("{c}. K2 changes nothing on a box whose sub-ids already overlap (rootless Docker, a hand-made range): fix them, then press Retry"),
        );
    }
    if ops.airgap() {
        return skip(
            "airgap",
            "air-gapped: K2 can't fetch its Podman or the App engine; the owner prepares with k2 app box prepare --podman-from and k2 app engine install --from",
        );
    }
    if let Some(msg) = k2_root_proto::apps::low_disk(&ops.disk_free()) {
        return skip("low_disk", msg);
    }
    if !ops.begin_busy() {
        return skip(
            "busy",
            "another root sequence (a hosted-mail enable or upgrade, the root helper's update, an owner's prep) is running; K2 tries again in 5 minutes",
        );
    }
    let _release = Release(ops);
    let out = steps(ops, prepare, engine);
    match out {
        Outcome::Failed { step, error } => match permanent_from_error(&error) {
            Some(code) => blocked(code, format!("{step}: {error}. K2 won't try again until you press Retry or K2 updates")),
            None => Outcome::Failed { step, error },
        },
        o => o,
    }
}

/// Prepare and the engine, inside the latch.
fn steps(ops: &dyn Ops, prepare: bool, engine: bool) -> Outcome {
    let mut changed: Vec<String> = Vec::new();
    if prepare {
        match ops.prepare() {
            Ok(v) => {
                if v["podman"]["action"] == "installed" {
                    changed.push(format!(
                        "installed K2's Podman {} in {} for the k2app user (k2 app box podman uninstall removes it)",
                        v["podman"]["version"].as_str().unwrap_or("?"),
                        v["podman"]["prefix"].as_str().unwrap_or("/opt/k2/podman")
                    ));
                }
                for s in v["steps"].as_array().cloned().unwrap_or_default() {
                    let verb = s["verb"].as_str().unwrap_or("").to_string();
                    if s["result"]["changed"] != json!(false) && verb != "apps podman install" {
                        changed.push(verb);
                    }
                }
            }
            Err(e) => {
                let step = e.split_once("box prep stopped at `").and_then(|(_, r)| r.split_once('`')).map(|(s, _)| s.to_string());
                return Outcome::Failed { step: step.unwrap_or_else(|| "box prepare".into()), error: e };
            }
        }
    }
    // The engine after the box: prepare may have just made its user.
    let (_, engine_now) = if prepare { needs(&ops.doctor()) } else { (false, engine) };
    if engine_now {
        match ops.install_engine() {
            Ok(v) => changed.push(format!(
                "installed the App engine {} (k2-app-engine, signed for this K2 version)",
                v["engine"]["version"].as_str().or(v["checked"]["version"].as_str()).unwrap_or("")
            )),
            Err(e) => return Outcome::Failed { step: "apps install-engine".into(), error: e },
        }
    }
    if !changed.is_empty() {
        ops.note_owner(
            "K2 prepared this server for Apps",
            &format!(
                "\"Prepare this box automatically\" is on, so after the update K2 set this server up for walled Apps through its \
                 root helper:\n\n- {}\n\nUndo: k2 app box podman uninstall (K2's Podman) and k2 app box unprep (the rest: Settings → \
                 Apps → This server → Undo box setup). Turn automatic prep off in Settings → Apps → This server (appsAutoPrepare).",
                changed.join("\n- ")
            ),
        );
    }
    Outcome::Prepared { changed }
}

/// Seconds until the next run after `o` (`None` = not until Retry or a K2
/// update); `backoff` is the failure backoff (1 h, doubling to 24 h).
pub fn next_wait(o: &Outcome, backoff: &mut u64) -> Option<u64> {
    match o {
        Outcome::Failed { .. } => {
            let w = *backoff;
            *backoff = (*backoff * 2).min(86_400);
            Some(w)
        }
        Outcome::Blocked { .. } => None,
        Outcome::Skipped { code, .. } if code == "busy" => Some(300),
        Outcome::Skipped { code, .. } if code == "helper_old" || code == "low_disk" => {
            *backoff = 3600;
            Some(6 * 3600)
        }
        _ => {
            *backoff = 3600;
            Some(86_400)
        }
    }
}

// ── status, kept across restarts (F7) ────────────────────────────────

/// `~/.k2/apps-auto-prepare.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Saved {
    pub last_run: Option<i64>,
    pub outcome: Option<J>,
    pub next_attempt: Option<i64>,
    /// The K2 version that recorded `outcome` (a block lifts on an update).
    pub version: Option<String>,
    /// The owner's "Undo box setup" (held until Retry).
    pub undone_at: Option<i64>,
    /// The owner pressed Retry since the last run.
    pub retry_requested: bool,
}

/// Where the status lives (tests use a scratch folder).
pub struct Store {
    pub path: PathBuf,
}

impl Store {
    pub fn live() -> Self {
        Store { path: k2_core::paths::k2_home().join("apps-auto-prepare.json") }
    }

    /// Missing or unreadable = never run (logged when unreadable).
    pub fn load(&self) -> Saved {
        match std::fs::read(&self.path) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|e| {
                k2_core::log_debug!("[apps] {}: {e}; starting fresh", self.path.display());
                Saved::default()
            }),
            Err(_) => Saved::default(),
        }
    }

    pub fn save(&self, s: &Saved) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?;
        k2_core::fs_atomic::atomic_write(&self.path, &bytes).map_err(|e| format!("{}: {e}", self.path.display()))
    }

    /// One run's result (the undo hold stays; a Retry is used up).
    pub fn record(&self, o: &Outcome, next: Option<i64>, now: i64, version: &str) -> Result<(), String> {
        let mut s = self.load();
        s.last_run = Some(now);
        s.outcome = Some(o.json());
        s.next_attempt = next;
        s.version = Some(version.to_string());
        s.retry_requested = false;
        self.save(&s)
    }

    /// The owner undid the box setup: hold automatic prep until Retry.
    pub fn mark_undone(&self, now: i64) -> Result<(), String> {
        let mut s = self.load();
        s.undone_at = Some(now);
        self.save(&s)
    }

    /// The owner's Retry: lift a block and the undo hold; run now.
    pub fn retry(&self, now: i64) -> Result<(), String> {
        let mut s = self.load();
        s.undone_at = None;
        s.retry_requested = true;
        s.next_attempt = Some(now);
        self.save(&s)
    }

    /// At boot: stay put (no run) while the last outcome is a block from
    /// this same K2 version and no Retry is pending.
    pub fn held_at_boot(&self, version: &str) -> bool {
        let s = self.load();
        let blocked = s.outcome.as_ref().and_then(Outcome::from_json).is_some_and(|o| o.is_blocked());
        blocked && s.version.as_deref() == Some(version) && !s.retry_requested
    }
}

/// What the doctor and Settings show, from `store`.
pub fn status_json_from(store: &Store) -> J {
    let s = store.load();
    let outcome = s.outcome.as_ref().and_then(Outcome::from_json);
    let mut out = json!({
        "enabled": k2_core::deploy::auto_prepare_enabled(),
        "default": k2_core::deploy::AUTO_PREPARE_DEFAULT,
        "setting": "appsAutoPrepare",
        "blocked": outcome.as_ref().is_some_and(Outcome::is_blocked) && !s.retry_requested,
        "undoneAt": s.undone_at,
        "retryRequested": s.retry_requested,
        "retryRoute": RETRY_ROUTE,
    });
    if let (Some(last), Some(o)) = (s.last_run, outcome) {
        out["lastRun"] = json!(last);
        out["outcome"] = o.json();
        out["nextAttempt"] = json!(s.next_attempt);
        out["version"] = json!(s.version);
    }
    out
}

/// What the doctor and Settings show.
pub fn status_json() -> J {
    status_json_from(&Store::live())
}

// ── waking the loop (Retry, the setting turned on) ───────────────────

struct Wake {
    flag: Mutex<bool>,
    cv: Condvar,
}

static WAKE: Wake = Wake { flag: Mutex::new(false), cv: Condvar::new() };

/// Run now (the loop is asleep, or waiting for a Retry).
pub fn wake() {
    *WAKE.flag.lock().unwrap_or_else(|p| p.into_inner()) = true;
    WAKE.cv.notify_all();
}

/// Sleep `secs` (`None` = until woken); a [`wake`] ends it early.
fn sleep_or_wake(secs: Option<u64>) {
    let deadline = secs.map(|s| Instant::now() + Duration::from_secs(s));
    let mut woke = WAKE.flag.lock().unwrap_or_else(|p| p.into_inner());
    loop {
        if *woke {
            *woke = false;
            return;
        }
        woke = match deadline {
            None => WAKE.cv.wait(woke).unwrap_or_else(|p| p.into_inner()),
            Some(d) => {
                let left = d.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return;
                }
                WAKE.cv.wait_timeout(woke, left).unwrap_or_else(|p| p.into_inner()).0
            }
        };
    }
}

/// `POST /cli/app/box/auto-prepare/retry {}` (owner): lift a block or the
/// undo hold and run now.
pub(crate) fn retry_route(who: &Who) -> R<J> {
    need_credential(who)?;
    if !matches!(who, Who::Owner | Who::Login { role: Role::Owner, .. }) {
        return Err(no("owner_only", 403, "retrying the automatic box prep is the owner's"));
    }
    let store = Store::live();
    store.retry(k2_core::deploy::now()).map_err(|e| no("state_write_failed", 500, e))?;
    super::grace::audit("app-auto-prepare-retry", &super::routes::actor(who), "retry requested");
    wake();
    Ok(json!({ "ok": true, "autoPrepare": status_json_from(&store) }))
}

// ── the live box ─────────────────────────────────────────────────────

struct LiveOps;

impl Ops for LiveOps {
    fn linux(&self) -> bool {
        cfg!(target_os = "linux")
    }
    fn apps_on(&self) -> bool {
        k2_core::deploy::enabled()
    }
    fn enabled(&self) -> bool {
        k2_core::deploy::auto_prepare_enabled()
    }
    fn undone(&self) -> bool {
        Store::live().load().undone_at.is_some()
    }
    fn helper_protocol(&self) -> Option<u32> {
        let st = crate::root_helper::state();
        st.usable(0).then_some(st.protocol)
    }
    fn doctor(&self) -> J {
        super::box_prep::doctor(&super::box_prep::gather())
    }
    fn subid_files(&self) -> (String, String) {
        (std::fs::read_to_string("/etc/subuid").unwrap_or_default(), std::fs::read_to_string("/etc/subgid").unwrap_or_default())
    }
    fn airgap(&self) -> bool {
        k2_core::airgap::enabled()
    }
    fn disk_free(&self) -> Vec<(String, u64)> {
        super::box_prep::disk_free_now()
    }
    fn begin_busy(&self) -> bool {
        crate::root_helper::busy::begin(LATCH_NAME)
    }
    fn end_busy(&self) {
        crate::root_helper::busy::end();
    }
    fn prepare(&self) -> Result<J, String> {
        let opts = super::box_prep::PrepareOpts { runtime: k2_root_proto::apps::OciRuntime::Crun, cores_mem_min_mib: None, budget: None };
        // The owner's setting is the opt-in to K2's Podman (lane P's
        // --install-podman); prepare installs it only where the box needs it.
        let pod = super::podman_install::PodmanOpts { install: true, dry_run: false, from: None };
        super::box_prep::prepare_core(&opts, &pod, ACTOR).map_err(|r| {
            let extra = r.extra.as_ref();
            let step = extra.and_then(|e| e["failedStep"].as_str()).unwrap_or("").to_string();
            if step.is_empty() {
                return format!("{}: {}", r.code, r.hint);
            }
            // The helper's own words (`podman_too_old: …`, `low_disk: …`):
            // what the doctor shows and what tells a block from a retry.
            let why = extra
                .and_then(|e| e["steps"].as_array())
                .and_then(|s| s.last())
                .and_then(|s| s["error"].as_str())
                .unwrap_or(&r.hint)
                .to_string();
            format!("box prep stopped at `{step}`: {why}")
        })
    }
    fn install_engine(&self) -> Result<J, String> {
        super::engine_install::install_core(None).map_err(|r| format!("{}: {}", r.code, r.hint))
    }
    fn note_owner(&self, title: &str, body: &str) {
        if let Err(e) = crate::feedback_routes::file_owner_notice(k2_core::db::AUDIT_SENTINEL_IDS[0], "k2-apps", title, body) {
            k2_core::log_debug!("[apps] auto-prepare notice: {e}");
        }
    }
}

/// Boot (= after every update): one detached thread on Linux.
pub fn spawn() {
    if cfg!(test) || !cfg!(target_os = "linux") {
        return;
    }
    let _ = std::thread::Builder::new().name("app-auto-prepare".into()).spawn(|| {
        let store = Store::live();
        let version = env!("CARGO_PKG_VERSION");
        // A block from this same version waits for Retry; an update lifts it.
        let mut wait = if store.held_at_boot(version) { None } else { Some(180) };
        let mut backoff = 3600u64;
        loop {
            sleep_or_wake(wait);
            let out = std::panic::catch_unwind(|| run_once(&LiveOps))
                .unwrap_or_else(|_| Outcome::Failed { step: "auto-prepare".into(), error: "the run panicked".into() });
            super::grace::audit("app-auto-prepare", ACTOR, &out.json().to_string());
            wait = next_wait(&out, &mut backoff);
            let now = k2_core::deploy::now();
            if let Err(e) = store.record(&out, wait.map(|w| now + w as i64), now, version) {
                k2_core::log_debug!("[apps] auto-prepare status: {e}");
            }
            k2_core::log_debug!("[apps] auto-prepare: {}; next in {wait:?}s", out.json());
        }
    });
}

#[cfg(test)]
#[path = "auto_prepare_tests.rs"]
mod tests;
