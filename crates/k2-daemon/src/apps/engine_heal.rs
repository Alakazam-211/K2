//! The App engine heal (prd-app-runtime-update-continuity-v1 R4, R5, R7,
//! A17, A18, A26; D1–D2): `k2-app-engine` updates together with K2.
//!
//! At every daemon boot (= after every update), when an engine is installed:
//! 1. read the installed file's build marker (cached by size and mtime in
//!    the status file; never at `/boot-status` time, A18) and the running
//!    engine's `version`;
//! 2. both at this daemon's version → `ok`;
//! 3. else wait for root helper protocol ≥ 6 (bounded 5 min), take the
//!    shared root latch, and install: the folder the updater or the
//!    installer staged (fetching a signature that wasn't published at stage
//!    time, A17), else the release download, else air-gapped → `needs_owner`
//!    with the `--from` hint;
//! 4. `engine_install::install_core` reads the bytes once, checks version
//!    and signature and sends **those bytes** to the helper (A26), and
//!    signals the supervisor (R43);
//! 5. re-check `version`: wrong → `failed`, one owner notice, backoff 1 h
//!    doubling to 24 h.
//!
//! Status lives in `~/.k2/apps-engine-heal.json`; `/boot-status`, `k2 app
//! engine status` and `GET /cli/app/box` show [`app_engine_json`] (R7).
//! Owner opt-out: `appsEngineHeal` / `K2_APP_ENGINE_HEAL=off` (R5).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use k2_root_proto::engine::{asset_name, ENGINE_PROTOCOL};
use k2_root_proto::version::APPS_MIN_PROTOCOL;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

use super::engine_stage::FetchError;

/// How long the heal waits for a usable root helper.
pub const HELPER_WAIT: Duration = Duration::from_secs(300);
/// The linger file box prep writes for `k2app` (A18).
pub const LINGER_FILE: &str = "/var/lib/systemd/linger/k2app";
/// R8: where the update job and `/boot-status` point for the re-host wave
/// (S2 serves it; its policy row is in `routes::route_policy`).
pub const REHOST_ROUTE: &str = "/cli/app/rehost";

/// A `(version, protocol)` pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Build {
    pub version: String,
    pub protocol: u64,
}

/// What one heal did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Installed and running engines match this daemon.
    Ok,
    /// Installed the matching engine (`source`: `staged` | `release`).
    Installed { source: String },
    /// Not run: `not_linux`, `apps_off`, `no_engine`, `setting_off`.
    Skipped { code: String },
    /// Only the owner can fix it (air-gapped with nothing staged; no helper).
    NeedsOwner { why: String },
    /// Try again after `after_secs` (a signature not published yet).
    Retry { why: String, after_secs: u64 },
    /// The root latch was held.
    Deferred,
    /// The install failed or the engine still doesn't match.
    Failed { why: String },
}

impl Outcome {
    pub fn json(&self) -> J {
        match self {
            Outcome::Ok => json!({ "state": "ok" }),
            Outcome::Installed { source } => json!({ "state": "installed", "source": source }),
            Outcome::Skipped { code } => json!({ "state": "skipped", "code": code }),
            Outcome::NeedsOwner { why } => json!({ "state": "needs_owner", "why": why }),
            Outcome::Retry { why, after_secs } => json!({ "state": "retry", "why": why, "afterSecs": after_secs }),
            Outcome::Deferred => json!({ "state": "deferred" }),
            Outcome::Failed { why } => json!({ "state": "failed", "why": why }),
        }
    }
}

/// Everything the heal touches, so tests never call the helper or the net.
pub trait HealOps {
    fn linux(&self) -> bool;
    fn apps_on(&self) -> bool;
    fn engine_installed(&self) -> bool;
    fn heal_enabled(&self) -> bool;
    /// The installed file's marker (cached).
    fn installed(&self) -> Result<Build, String>;
    /// The running engine's `version` answer.
    fn running(&self) -> Result<J, String>;
    fn helper_protocol(&self) -> Option<u32>;
    fn sleep(&self, d: Duration);
    fn begin_busy(&self) -> bool;
    fn end_busy(&self);
    fn airgap(&self) -> bool;
    fn fetch(&self, url: &str) -> Result<Vec<u8>, FetchError>;
    /// `engine_install::install_core(from)`: `Err((code, hint))`.
    fn install(&self, from: Option<&Path>) -> Result<J, (String, String)>;
    fn note_owner(&self, title: &str, body: &str);
}

/// The inputs that aren't side effects.
#[derive(Debug, Clone)]
pub struct Plan {
    pub want: String,
    pub arch: String,
    /// `…/releases/download`.
    pub base_url: String,
    /// `staged_dir(want)`.
    pub staged: PathBuf,
}

fn running_build(v: &J) -> Build {
    Build { version: v["version"].as_str().unwrap_or("?").to_string(), protocol: v["protocol"].as_u64().unwrap_or(0) }
}

/// What the heal saw before deciding (the status file's numbers).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seen {
    pub installed: Option<Build>,
    pub running: Option<Build>,
}

fn matches(b: &Option<Build>, want: &str) -> bool {
    b.as_ref().is_some_and(|b| b.version == want && b.protocol == ENGINE_PROTOCOL as u64)
}

/// One heal. `seen` gets what it read (even when it then skips).
pub fn heal_once(ops: &dyn HealOps, plan: &Plan, seen: &mut Seen) -> Outcome {
    if !ops.linux() {
        return Outcome::Skipped { code: "not_linux".into() };
    }
    if !ops.engine_installed() {
        return Outcome::Skipped { code: "no_engine".into() };
    }
    seen.installed = ops.installed().ok();
    // An engine starting with the box may need a moment to answer.
    let mut running = ops.running();
    for _ in 0..15 {
        if running.is_ok() {
            break;
        }
        ops.sleep(Duration::from_secs(2));
        running = ops.running();
    }
    seen.running = running.as_ref().ok().map(running_build);
    if !ops.apps_on() {
        return Outcome::Skipped { code: "apps_off".into() };
    }
    if !ops.heal_enabled() {
        return Outcome::Skipped { code: "setting_off".into() };
    }
    if matches(&seen.installed, &plan.want) && matches(&seen.running, &plan.want) {
        return Outcome::Ok;
    }
    // The helper (protocol 6 = `apps install-engine`), bounded.
    let mut waited = Duration::ZERO;
    while !ops.helper_protocol().is_some_and(|p| p >= APPS_MIN_PROTOCOL) {
        if waited >= HELPER_WAIT {
            return Outcome::NeedsOwner {
                why: format!("the root helper isn't installed at protocol {APPS_MIN_PROTOCOL}+ (k2 root-helper status)"),
            };
        }
        ops.sleep(Duration::from_secs(10));
        waited += Duration::from_secs(10);
    }
    if !ops.begin_busy() {
        return Outcome::Deferred;
    }
    let out = install_step(ops, plan, seen);
    ops.end_busy();
    out
}

fn install_step(ops: &dyn HealOps, plan: &Plan, seen: &mut Seen) -> Outcome {
    let name = asset_name(&plan.arch);
    let staged_bin = plan.staged.join(&name).is_file();
    let sig_path = plan.staged.join(format!("{name}.sig"));
    let (from, source) = if staged_bin {
        if !sig_path.is_file() {
            // A17: staged before the signature was published.
            let url = format!("{}/v{}/{name}.sig", plan.base_url, plan.want);
            match ops.fetch(&url) {
                Ok(b) => {
                    if let Err(e) = std::fs::write(&sig_path, &b) {
                        return Outcome::Failed { why: format!("{}: {e}", sig_path.display()) };
                    }
                }
                Err(FetchError::NotFound) if !ops.airgap() => {
                    return Outcome::Retry { why: "the engine's signature is not published yet".into(), after_secs: 600 };
                }
                Err(FetchError::NotFound) => {
                    return Outcome::NeedsOwner { why: format!("air-gapped: copy {name}.sig into {}", plan.staged.display()) };
                }
                Err(FetchError::Other(e)) => return Outcome::Retry { why: format!("signature: {e}"), after_secs: 3600 },
            }
        }
        (Some(plan.staged.as_path()), "staged")
    } else if ops.airgap() {
        return Outcome::NeedsOwner {
            why: format!("air-gapped: copy {name} and {name}.sig for v{} into a folder and run k2 app engine install --from <folder>", plan.want),
        };
    } else {
        (None, "release")
    };
    match ops.install(from) {
        Ok(_) => {}
        Err((code, hint)) if code == "engine_download_failed" => return Outcome::Retry { why: hint, after_secs: 3600 },
        Err((code, hint)) => {
            let why = format!("{code}: {hint}");
            note_failed(ops, plan, &why);
            return Outcome::Failed { why };
        }
    }
    let after = ops.running().ok().map(|v| running_build(&v));
    seen.running = after.clone();
    if !matches(&after, &plan.want) {
        let why = format!(
            "installed, but the running engine is {} (want v{} protocol {ENGINE_PROTOCOL})",
            after.map(|b| format!("v{} protocol {}", b.version, b.protocol)).unwrap_or_else(|| "not answering".into()),
            plan.want
        );
        note_failed(ops, plan, &why);
        return Outcome::Failed { why };
    }
    seen.installed = Some(Build { version: plan.want.clone(), protocol: ENGINE_PROTOCOL as u64 });
    Outcome::Installed { source: source.into() }
}

fn note_failed(ops: &dyn HealOps, plan: &Plan, why: &str) {
    super::runtime_notes::error(&format!("engine heal to v{} failed: {why}", plan.want));
    ops.note_owner(
        "K2 couldn't update its App engine",
        &format!(
            "K2 tried to install k2-app-engine v{} to match this K2 and it didn't work: {why}. Apps that should run wait \
             for the engine. K2 tries again in an hour; the owner can run k2 app engine install now.",
            plan.want
        ),
    );
}

/// Seconds until the next heal.
pub fn next_wait(o: &Outcome, backoff: &mut u64) -> u64 {
    match o {
        Outcome::Retry { after_secs, .. } => *after_secs,
        Outcome::Deferred => 300,
        Outcome::NeedsOwner { .. } => 3600,
        Outcome::Failed { .. } => {
            let w = *backoff;
            *backoff = (*backoff * 2).min(86_400);
            w
        }
        _ => {
            *backoff = 3600;
            86_400
        }
    }
}

// ── status ───────────────────────────────────────────────────────────

/// `~/.k2/apps-engine-heal.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub last_run: Option<i64>,
    pub outcome: Option<J>,
    pub installed: Option<Build>,
    pub running: Option<Build>,
    pub want: Option<String>,
    pub next_attempt: Option<i64>,
    /// The marker cache: the file's size and mtime when it was read.
    pub marker_size: Option<u64>,
    pub marker_mtime: Option<i64>,
}

fn status_path() -> PathBuf {
    k2_core::paths::k2_home().join("apps-engine-heal.json")
}

fn status_mem() -> &'static Mutex<Option<Status>> {
    static S: Mutex<Option<Status>> = Mutex::new(None);
    &S
}

static HEALING: AtomicBool = AtomicBool::new(false);

pub fn load_status() -> Status {
    let mut mem = status_mem().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = mem.clone() {
        return s;
    }
    let s: Status = std::fs::read(status_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    *mem = Some(s.clone());
    s
}

fn save_status(s: &Status) {
    *status_mem().lock().unwrap_or_else(|e| e.into_inner()) = Some(s.clone());
    let p = status_path();
    if let Ok(b) = serde_json::to_vec_pretty(s) {
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, b).and_then(|_| std::fs::rename(&tmp, &p)).is_err() {
            k2_core::log_debug!("[apps] engine heal: couldn't write {}", p.display());
        }
    }
}

/// R7: the `/boot-status` `appEngine` block from its parts (pure).
#[allow(clippy::too_many_arguments)]
pub fn compose(st: &Status, gate: &super::engine_gate::GateCore, linger: bool, waiting: usize, want: &str, healing: bool) -> J {
    let installed = st.installed.clone();
    let running = gate
        .running
        .as_ref()
        .map(|r| Build { version: r.version.clone(), protocol: r.protocol })
        .or_else(|| if gate.last_error.is_some() { None } else { st.running.clone() });
    let off = |b: &Option<Build>| b.as_ref().is_some_and(|b| b.version != want || b.protocol != ENGINE_PROTOCOL as u64);
    let mismatch = off(&installed) || off(&running);
    let heal_state = st.outcome.as_ref().and_then(|o| o["state"].as_str()).unwrap_or("");
    let state = if healing {
        "healing"
    } else if gate.not_answering {
        "not_answering"
    } else if mismatch && heal_state == "failed" {
        "failed"
    } else if mismatch && heal_state == "needs_owner" {
        "needs_owner"
    } else if mismatch {
        "mismatch"
    } else if !linger {
        "needs_owner"
    } else {
        "ok"
    };
    let b = |x: &Option<Build>| x.as_ref().map(|b| json!({ "version": b.version, "protocol": b.protocol }));
    json!({
        "state": state,
        "want": { "version": want, "protocol": ENGINE_PROTOCOL },
        "installed": b(&installed),
        "running": b(&running),
        "linger": linger,
        "heal": st.outcome,
        "lastRun": st.last_run,
        "nextAttempt": st.next_attempt,
        // S2 fills the re-host counts (runtime fingerprints, 0221).
        "apps": { "onOldEngine": null, "rehostPending": null, "waitingEngine": waiting },
    })
}

/// R7: the live block, or `None` with no engine installed (`/boot-status`
/// stays byte-identical on such boxes).
pub fn app_engine_json() -> Option<J> {
    if !super::guarded::engine_installed() {
        return None;
    }
    // `/boot-status` answers before the DB is open: count only once it is.
    let waiting = k2_core::db::try_shared()
        .map(|h| {
            let c = h.lock();
            c.query_row("SELECT COUNT(*) FROM apps WHERE state = ?1", [k2_core::deploy::store::STATE_WAITING_ENGINE], |r| r.get::<_, i64>(0))
                .unwrap_or(0) as usize
        })
        .unwrap_or(0);
    let mut v = compose(
        &load_status(),
        &super::engine_gate::snapshot(),
        Path::new(LINGER_FILE).exists(),
        waiting,
        env!("CARGO_PKG_VERSION"),
        HEALING.load(Ordering::SeqCst),
    );
    // S2: Apps still on an older runtime, and the wave's pending ones.
    if let Some((old, pending)) = super::rehost::counts() {
        v["apps"]["onOldEngine"] = json!(old);
        v["apps"]["rehostPending"] = json!(pending);
    }
    Some(v)
}

/// `/boot-status` full view: add `appEngine` only when an engine is
/// installed (default body unchanged).
pub fn decorate_boot_status(body: J) -> J {
    decorate_with(body, app_engine_json())
}

fn decorate_with(mut body: J, engine: Option<J>) -> J {
    if let (Some(e), Some(obj)) = (engine, body.as_object_mut()) {
        obj.insert("appEngine".into(), e);
        // R8: where the update job reads the re-host wave (S2's route).
        obj.insert("appRehost".into(), J::String(REHOST_ROUTE.into()));
    }
    body
}

/// `GET /cli/app/engine/status` (R9): any credential that can read Apps.
pub(crate) fn status_route() -> super::routes::R<J> {
    Ok(app_engine_json().unwrap_or_else(|| json!({ "state": "absent", "linger": Path::new(LINGER_FILE).exists() })))
}

// ── live ─────────────────────────────────────────────────────────────

struct LiveHeal;

impl HealOps for LiveHeal {
    fn linux(&self) -> bool {
        cfg!(target_os = "linux")
    }
    fn apps_on(&self) -> bool {
        k2_core::deploy::enabled()
    }
    fn engine_installed(&self) -> bool {
        super::guarded::engine_installed()
    }
    fn heal_enabled(&self) -> bool {
        k2_core::deploy::engine_heal_enabled()
    }
    fn installed(&self) -> Result<Build, String> {
        let p = Path::new(k2_root_proto::apps::ENGINE_PATH);
        let md = std::fs::metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let mtime = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64);
        let st = load_status();
        if let (Some(b), Some(sz), Some(mt)) = (st.installed.clone(), st.marker_size, st.marker_mtime) {
            if sz == md.len() && Some(mt) == mtime {
                return Ok(b);
            }
        }
        let (version, protocol) = super::engine_install::installed_marker(p)?;
        let b = Build { version, protocol: protocol as u64 };
        let mut st = st;
        st.installed = Some(b.clone());
        st.marker_size = Some(md.len());
        st.marker_mtime = mtime;
        save_status(&st);
        Ok(b)
    }
    fn running(&self) -> Result<J, String> {
        let ans = super::guarded::engine_version();
        super::engine_gate::observe(&ans);
        ans
    }
    fn helper_protocol(&self) -> Option<u32> {
        let st = crate::root_helper::state();
        st.usable(0).then_some(st.protocol)
    }
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
    fn begin_busy(&self) -> bool {
        crate::root_helper::busy::begin("the App engine heal")
    }
    fn end_busy(&self) {
        crate::root_helper::busy::end();
    }
    fn airgap(&self) -> bool {
        k2_core::airgap::enabled()
    }
    fn fetch(&self, url: &str) -> Result<Vec<u8>, FetchError> {
        super::engine_stage::live_fetch(url)
    }
    fn install(&self, from: Option<&Path>) -> Result<J, (String, String)> {
        HEALING.store(true, Ordering::SeqCst);
        let from_s = from.map(|p| p.to_string_lossy().into_owned());
        let r = super::engine_install::install_core(from_s.as_deref()).map_err(|r| (r.code.to_string(), r.hint));
        HEALING.store(false, Ordering::SeqCst);
        r
    }
    fn note_owner(&self, title: &str, body: &str) {
        super::runtime_notes::owner_notice(title, body);
    }
}

pub fn live_plan() -> Plan {
    let want = env!("CARGO_PKG_VERSION").to_string();
    Plan {
        staged: super::engine_stage::staged_dir(&want),
        want,
        arch: std::env::consts::ARCH.to_string(),
        base_url: crate::root_helper::RELEASE_DOWNLOAD_BASE.to_string(),
    }
}

/// Boot: one thread (Linux only), at once (Apps wait on the engine), then
/// daily, sooner on a retry or failure.
pub fn spawn() {
    if cfg!(test) || !cfg!(target_os = "linux") {
        return;
    }
    let _ = std::thread::Builder::new().name("apps-engine-heal".into()).spawn(|| {
        let mut backoff = 3600u64;
        loop {
            let plan = live_plan();
            let mut seen = Seen::default();
            let out = heal_once(&LiveHeal, &plan, &mut seen);
            let wait = next_wait(&out, &mut backoff);
            let mut st = load_status();
            let now = k2_core::deploy::now();
            st.last_run = Some(now);
            st.outcome = Some(out.json());
            if seen.installed.is_some() {
                st.installed = seen.installed.clone();
            }
            st.running = seen.running.clone();
            st.want = Some(plan.want.clone());
            st.next_attempt = Some(now + wait as i64);
            save_status(&st);
            k2_core::log_debug!("[apps] engine heal: {}; next in {wait}s", out.json());
            std::thread::sleep(Duration::from_secs(wait));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    struct Fake {
        installed: RefCell<Build>,
        running: RefCell<Result<J, String>>,
        helper: Option<u32>,
        airgap: bool,
        enabled: bool,
        net: RefCell<std::collections::HashMap<String, Vec<u8>>>,
        installs: RefCell<Vec<Option<PathBuf>>>,
        install_err: Option<(String, String)>,
        /// What a successful install makes the engine report.
        after: Build,
        notes: RefCell<Vec<String>>,
        busy_free: Cell<bool>,
    }

    impl Fake {
        fn new(installed: (&str, u64), running: (&str, u64)) -> Self {
            Fake {
                installed: RefCell::new(Build { version: installed.0.into(), protocol: installed.1 }),
                running: RefCell::new(Ok(json!({ "version": running.0, "protocol": running.1 }))),
                helper: Some(9),
                airgap: false,
                enabled: true,
                net: RefCell::new(Default::default()),
                installs: RefCell::new(vec![]),
                install_err: None,
                after: Build { version: "0.46.4".into(), protocol: ENGINE_PROTOCOL as u64 },
                notes: RefCell::new(vec![]),
                busy_free: Cell::new(true),
            }
        }
    }

    impl HealOps for Fake {
        fn linux(&self) -> bool {
            true
        }
        fn apps_on(&self) -> bool {
            true
        }
        fn engine_installed(&self) -> bool {
            true
        }
        fn heal_enabled(&self) -> bool {
            self.enabled
        }
        fn installed(&self) -> Result<Build, String> {
            Ok(self.installed.borrow().clone())
        }
        fn running(&self) -> Result<J, String> {
            self.running.borrow().clone()
        }
        fn helper_protocol(&self) -> Option<u32> {
            self.helper
        }
        fn sleep(&self, _d: Duration) {}
        fn begin_busy(&self) -> bool {
            self.busy_free.get()
        }
        fn end_busy(&self) {}
        fn airgap(&self) -> bool {
            self.airgap
        }
        fn fetch(&self, url: &str) -> Result<Vec<u8>, FetchError> {
            self.net.borrow().get(url).cloned().ok_or(FetchError::NotFound)
        }
        fn install(&self, from: Option<&Path>) -> Result<J, (String, String)> {
            self.installs.borrow_mut().push(from.map(Path::to_path_buf));
            if let Some(e) = &self.install_err {
                return Err(e.clone());
            }
            *self.installed.borrow_mut() = self.after.clone();
            *self.running.borrow_mut() = Ok(json!({ "version": self.after.version, "protocol": self.after.protocol }));
            Ok(json!({ "ok": true }))
        }
        fn note_owner(&self, title: &str, _body: &str) {
            self.notes.borrow_mut().push(title.into());
        }
    }

    fn plan(tag: &str) -> Plan {
        let d = std::env::temp_dir().join(format!("k2-engine-heal-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        Plan { want: "0.46.4".into(), arch: "x86_64".into(), base_url: "https://rel".into(), staged: d }
    }

    fn stage_bin(p: &Plan, sig: bool) {
        std::fs::create_dir_all(&p.staged).unwrap();
        std::fs::write(p.staged.join("k2-app-engine-linux-x86_64"), b"bin").unwrap();
        if sig {
            std::fs::write(p.staged.join("k2-app-engine-linux-x86_64.sig"), b"sig").unwrap();
        }
    }

    /// T1: the decision table and the opt-out.
    #[test]
    fn heal_decision_table() {
        let p = plan("t1");
        let mut s = Seen::default();
        // Both match: nothing to do, no install.
        let f = Fake::new(("0.46.4", 3), ("0.46.4", 3));
        assert_eq!(heal_once(&f, &p, &mut s), Outcome::Ok);
        assert!(f.installs.borrow().is_empty());
        // A stale FILE behind a matching process (0.46.3 file, process
        // somehow current) and a stale PROCESS behind a fresh file both heal.
        for (inst, run) in [(("0.46.3", 3), ("0.46.4", 3)), (("0.46.4", 3), ("0.46.0", 2))] {
            let f = Fake::new(inst, run);
            assert_eq!(heal_once(&f, &p, &mut s), Outcome::Installed { source: "release".into() }, "{inst:?} {run:?}");
            assert_eq!(*f.installs.borrow(), vec![None], "from the release (nothing staged)");
        }
        // The owner's opt-out: seen is still filled for status.
        let mut f = Fake::new(("0.46.0", 2), ("0.46.0", 2));
        f.enabled = false;
        let mut s = Seen::default();
        assert_eq!(heal_once(&f, &p, &mut s), Outcome::Skipped { code: "setting_off".into() });
        assert_eq!(s.running, Some(Build { version: "0.46.0".into(), protocol: 2 }));
        assert!(f.installs.borrow().is_empty());
    }

    /// T2: staged → release → air-gap; helper wait → needs_owner; the root
    /// latch; a failed re-check notices the owner.
    #[test]
    fn heal_order_and_refusals() {
        let p = plan("t2");
        stage_bin(&p, true);
        let f = Fake::new(("0.46.3", 3), ("0.46.3", 3));
        let mut s = Seen::default();
        assert_eq!(heal_once(&f, &p, &mut s), Outcome::Installed { source: "staged".into() });
        assert_eq!(*f.installs.borrow(), vec![Some(p.staged.clone())], "the staged folder (install_core reads its bytes once)");
        assert_eq!(s.installed, Some(Build { version: "0.46.4".into(), protocol: 3 }));

        let p2 = plan("t2-air");
        let mut f = Fake::new(("0.46.3", 3), ("0.46.3", 3));
        f.airgap = true;
        assert!(matches!(heal_once(&f, &p2, &mut s), Outcome::NeedsOwner { why } if why.contains("--from")));
        assert!(f.installs.borrow().is_empty());

        let mut f = Fake::new(("0.46.3", 3), ("0.46.3", 3));
        f.helper = Some(5);
        assert!(matches!(heal_once(&f, &p2, &mut s), Outcome::NeedsOwner { why } if why.contains("protocol 6")));

        let f = Fake::new(("0.46.3", 3), ("0.46.3", 3));
        f.busy_free.set(false);
        assert_eq!(heal_once(&f, &p2, &mut s), Outcome::Deferred);
        assert!(f.installs.borrow().is_empty());

        let mut f = Fake::new(("0.46.3", 3), ("0.46.3", 3));
        f.after = Build { version: "0.46.3".into(), protocol: 3 };
        assert!(matches!(heal_once(&f, &p2, &mut s), Outcome::Failed { why } if why.contains("running engine is v0.46.3")));
        assert_eq!(*f.notes.borrow(), vec!["K2 couldn't update its App engine".to_string()]);

        let mut f = Fake::new(("0.46.3", 3), ("0.46.3", 3));
        f.install_err = Some(("engine_refused".into(), "the signed engine is v0.46.2".into()));
        assert!(matches!(heal_once(&f, &p2, &mut s), Outcome::Failed { .. }));
        let mut back = 3600;
        assert_eq!((next_wait(&Outcome::Failed { why: String::new() }, &mut back), back), (3600, 7200));
        std::fs::remove_dir_all(&p.staged).unwrap();
    }

    /// T33 (A17), the heal half: a staged engine with no signature fetches
    /// it once published; until then it retries in 10 minutes.
    #[test]
    fn a_missing_signature_is_fetched_before_install() {
        let p = plan("t33");
        stage_bin(&p, false);
        let f = Fake::new(("0.46.3", 3), ("0.46.3", 3));
        let mut s = Seen::default();
        assert_eq!(heal_once(&f, &p, &mut s), Outcome::Retry { why: "the engine's signature is not published yet".into(), after_secs: 600 });
        assert!(f.installs.borrow().is_empty());
        f.net.borrow_mut().insert("https://rel/v0.46.4/k2-app-engine-linux-x86_64.sig".into(), b"SIG".to_vec());
        assert_eq!(heal_once(&f, &p, &mut s), Outcome::Installed { source: "staged".into() });
        assert_eq!(std::fs::read(p.staged.join("k2-app-engine-linux-x86_64.sig")).unwrap(), b"SIG");
        std::fs::remove_dir_all(&p.staged).unwrap();
    }

    /// T4: no engine → byte-identical body; stale file or process →
    /// `mismatch`; no linger → `needs_owner`; healing and not answering win.
    #[test]
    fn boot_status_app_engine_block() {
        let body = json!({ "version": "0.46.4", "phase": "ready" });
        assert_eq!(decorate_with(body.clone(), None).to_string(), body.to_string(), "byte-identical with no engine");
        let mut g = super::super::engine_gate::GateCore::default();
        let ok = |p: u64| json!({ "version": "0.46.4", "protocol": p });
        g.observe(&Ok(ok(3)), 3);
        let st = Status { installed: Some(Build { version: "0.46.4".into(), protocol: 3 }), ..Default::default() };
        let v = compose(&st, &g, true, 0, "0.46.4", false);
        assert_eq!(v["state"], "ok");
        assert_eq!(v["running"]["protocol"], 3);
        assert_eq!(compose(&st, &g, false, 0, "0.46.4", false)["state"], "needs_owner", "no linger");
        let stale_file = Status { installed: Some(Build { version: "0.46.3".into(), protocol: 3 }), ..Default::default() };
        assert_eq!(compose(&stale_file, &g, true, 0, "0.46.4", false)["state"], "mismatch");
        let mut g2 = super::super::engine_gate::GateCore::default();
        g2.observe(&Ok(json!({ "version": "0.46.0", "protocol": 2 })), 3);
        let v = compose(&st, &g2, true, 2, "0.46.4", false);
        assert_eq!((v["state"].as_str(), v["apps"]["waitingEngine"].as_u64()), (Some("mismatch"), Some(2)), "a stale process");
        assert_eq!(compose(&st, &g2, true, 0, "0.46.4", true)["state"], "healing");
        g2.not_answering = true;
        assert_eq!(compose(&st, &g2, true, 0, "0.46.4", false)["state"], "not_answering");
        let failed = Status { outcome: Some(json!({ "state": "failed" })), ..st.clone() };
        let mut g3 = super::super::engine_gate::GateCore::default();
        g3.observe(&Ok(json!({ "version": "0.46.0", "protocol": 2 })), 3);
        assert_eq!(compose(&failed, &g3, true, 0, "0.46.4", false)["state"], "failed");
        let d = decorate_with(body, Some(v));
        assert_eq!(d["appRehost"], "/cli/app/rehost");
    }
}
