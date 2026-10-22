//! The deploy pipeline and the App supervisor (§7.2–7.3, §6.3; S7).
//!
//! A deploy: verify the Seed → start it on a fresh port → health gate →
//! **switch** (the daemon-local route override; never a control-plane
//! write) → the old instance drains and stays **warm** through the
//! **watch window** → when the window passes, the warm one stops.
//!
//! During the window a crash or failing health checks **auto-roll back**
//! to the warm instance. After it, a crash restarts the live Seed with K2
//! backoff (at once, 5 s, 30 s, 2 min, 10 min, 30 min) and the crash-loop
//! breaker (6 restarts in an hour → `crashlooping`, the App's agent told).
//! A deploy that isn't live after `stall_secs` (or that a daemon restart
//! interrupted) is `stalled` and the App's agent is told (litmus L3).
//!
//! One background tick thread drives the watch, health and restarts. Every
//! step writes `deployments` and the hash-chained `deploy_audit`. The
//! leak-aware rollback (memory slope) joins in stage R (S18).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_core::deploy::backoff::{Backoff, Decision};
use k2_core::deploy::engine::{AppSpec, Isolation, Liveness, RunHandle, RuntimeEngine, SeedRef};
use k2_core::deploy::health::{self, ProbeAs};
use k2_core::deploy::manifest::Manifest;
use k2_core::deploy::store::{self, AppRow, DeploymentRow, NewDeployment};
use k2_core::deploy::{self, audit, codes, Timings};
use serde_json::json;

use super::process_engine::ProcessEngine;
use super::router;

/// Health failures in a row that count as "unhealthy" (watch and after).
const FAILS_TO_UNHEALTHY: u32 = 3;
/// An unwalled instance re-adopted after a daemon restart is probed every
/// second for this long (smoke r6: not once a minute). Failing it is a
/// relaunch, never a crash.
const ADOPT_CHECK_SECS: u64 = 15;
/// Why a restart the daemon's own restart caused is not a crash.
const WHY_DAEMON_RESTARTED: &str = "the K2 daemon restarted";
const WHY_STALE_SINK: &str = "relaunch: its log pipe went with the old K2 daemon";
const WHY_ADOPT_UNHEALTHY: &str = "relaunch: it didn't answer its health check after the K2 daemon restarted";
/// Deployments of an App kept for rollback targets and the sweep.
pub const KEEP_DEPLOYMENTS: usize = 10;

#[derive(Debug, Clone)]
pub struct Instance {
    pub handle: RunHandle,
    pub seed: String,
    pub health_path: String,
    pub probe_as: ProbeAs,
    /// The check is a TCP accept (K5: an old `--cmd` service on the open
    /// path).
    pub probe_tcp: bool,
    pub drain_secs: u64,
    /// `host.clientIp` (0.46.3; default on): the router's client-IP rewrite.
    pub client_ip: bool,
}

#[derive(Debug, Clone)]
struct Watch {
    deployment_id: String,
    until: Instant,
}

#[derive(Debug, Default)]
struct AppRt {
    live: Option<Instance>,
    warm: Option<Instance>,
    watch: Option<Watch>,
    backoff: Backoff,
    busy: bool,
    next_restart_at: Option<i64>,
    live_fails: u32,
    last_probe: Option<Instant>,
    /// Why the pending restart happens (`None`: a crash restart).
    restart_why: Option<&'static str>,
    /// A re-adopted unwalled instance is being checked (every second)
    /// until this deadline.
    adopt_check: Option<Instant>,
    /// Start a fresh instance of the live Seed now (not a crash).
    relaunch: Option<&'static str>,
    /// The next instance to go live replaces the live one outright (no
    /// warm copy, no rollback to it).
    retire_on_live: bool,
    /// The deployment whose pipeline thread is running (set once its row
    /// exists, cleared after `busy`): [`wait_done`] returns only once it
    /// is gone, so a caller's next verb never meets this one's lock.
    in_flight: Option<String>,
}

fn state() -> &'static Mutex<HashMap<String, AppRt>> {
    static S: OnceLock<Mutex<HashMap<String, AppRt>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn with_rt<R>(app_id: &str, f: impl FnOnce(&mut AppRt) -> R) -> R {
    let mut s = state().lock().unwrap_or_else(|e| e.into_inner());
    f(s.entry(app_id.to_string()).or_default())
}

/// The process engine (stage 1; the default isolation).
pub fn engine() -> &'static dyn RuntimeEngine {
    static E: ProcessEngine = ProcessEngine;
    &E
}

/// The engine for an isolation (`None`: no engine on this box gives it;
/// the deploy refuses instead of falling back, AR24). The unwalled process
/// engine only when the owner opted into it (`deploy::process_engine_enabled`,
/// 0.46.3): the default-on Apps switch never runs App code unwalled.
pub fn engine_for(iso: Isolation) -> Option<&'static dyn RuntimeEngine> {
    match iso {
        Isolation::Process if deploy::process_engine_enabled() => Some(engine()),
        Isolation::Container if cfg!(target_os = "linux") => Some(super::guarded::engine()),
        _ => None,
    }
}

/// The engine for an App: its isolation, and for `process` its path —
/// the open path's `process` exception (owner-approved, a sandboxed
/// system unit as `k2appx`) is not stage 1's unwalled process engine.
pub fn engine_for_app(app: &AppRow) -> Option<&'static dyn RuntimeEngine> {
    if app.is_process_exception() {
        return cfg!(target_os = "linux").then(|| super::process_exception::engine() as &'static dyn RuntimeEngine);
    }
    if app.path == "open" {
        // S13: the open path's relaxed container. `open` + `process`
        // without the owner's exception has no engine (AR24: never run an
        // App behind a different wall than asked).
        return match Isolation::parse(&app.isolation) {
            Some(Isolation::Container) if cfg!(target_os = "linux") => Some(super::open::engine()),
            _ => None,
        };
    }
    engine_for(Isolation::parse(&app.isolation).unwrap_or(Isolation::Process))
}

/// Would this App run on the unwalled stage-1 process engine (guarded path,
/// isolation `process`, not the owner's process exception)?
pub fn runs_unwalled(app: &AppRow) -> bool {
    !app.is_process_exception()
        && app.path != "open"
        && Isolation::parse(&app.isolation).unwrap_or(Isolation::Process) == Isolation::Process
}

/// The state line an unwalled App gets when the process engine is off.
const UNWALLED_STOPPED: &str = "stopped: the unwalled process engine is off on this box (deploy it guarded: --engine guarded)";

/// The process engine is off: stop every App instance it runs (live and
/// warm) and mark those Apps stopped with the reason. No-op while it's on
/// or before the supervisor started. Returns the Apps it stopped.
pub fn stop_unwalled_if_off(actor: &str) -> Vec<String> {
    if deploy::process_engine_enabled() {
        return Vec::new();
    }
    let unwalled = |i: &Option<Instance>| i.as_ref().is_some_and(|i| is_process_engine(&i.handle));
    let ids: Vec<String> = state()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|(_, rt)| unwalled(&rt.live) || unwalled(&rt.warm))
        .map(|(id, _)| id.clone())
        .collect();
    let mut stopped = Vec::new();
    for id in ids {
        let Ok(Some(app)) = db(|c| store::get_app(c, &id)) else { continue };
        match stop_app(&app, actor) {
            Ok(()) => {
                let _ = db(|c| store::set_state(c, &id, store::STATE_STOPPED, Some(UNWALLED_STOPPED)));
                audit(&id, "process_engine_off", actor, json!({ "stopped": true }));
                stopped.push(id);
            }
            Err(f) => k2_core::log_debug!("[apps] {id}: stopping it (process engine off): {}", f.message),
        }
    }
    stopped
}

/// [`stop_unwalled_if_off`] on its own thread (a settings write).
pub fn spawn_stop_unwalled_if_off(actor: &'static str) {
    if deploy::process_engine_enabled() {
        return;
    }
    let _ = std::thread::Builder::new().name("apps-unwalled-stop".into()).spawn(move || {
        let _ = stop_unwalled_if_off(actor);
    });
}

fn is_process_engine(h: &RunHandle) -> bool {
    !super::open::owns(h) && !super::guarded::owns(h) && !super::process_exception::owns(h)
}

/// The engine that runs an instance (by its handle).
pub fn engine_of(h: &RunHandle) -> &'static dyn RuntimeEngine {
    if super::open::owns(h) {
        super::open::engine()
    } else if super::guarded::owns(h) {
        super::guarded::engine()
    } else if super::process_exception::owns(h) {
        super::process_exception::engine()
    } else {
        engine()
    }
}

/// The App front listeners and their splices run on the supervisor's own
/// small runtime, so they never depend on (or die with) a caller's runtime.
fn runtime() -> Option<&'static tokio::runtime::Runtime> {
    static RT: OnceLock<Option<tokio::runtime::Runtime>> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("apps-front")
            .enable_all()
            .build()
            .map_err(|e| k2_core::log_debug!("[apps] front runtime: {e}"))
            .ok()
    })
    .as_ref()
}

pub fn timings() -> Timings {
    Timings::from_env()
}

fn now() -> i64 {
    deploy::now()
}

/// A refusal the routes turn into JSON.
#[derive(Debug, Clone)]
pub struct Fail {
    pub code: &'static str,
    pub status: u16,
    pub message: String,
}

fn fail(code: &'static str, status: u16, message: impl Into<String>) -> Fail {
    Fail { code, status, message: message.into() }
}

fn db<R>(f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<R>) -> Result<R, Fail> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    f(&conn).map_err(|e| fail("db_error", 500, e.to_string()))
}

fn audit(app_id: &str, event: &str, actor: &str, payload: serde_json::Value) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    if let Err(e) = audit::append(&conn, audit::SUBJECT_APP, app_id, event, actor, None, &payload) {
        k2_core::log_debug!("[apps] audit {app_id} {event}: {e}");
    }
}

// ── start-up ─────────────────────────────────────────────────────────

/// Start the supervisor once per process: re-adopt running instances,
/// mark interrupted deploys stalled, open front listeners, start the tick.
/// Called at daemon boot and lazily by the routes (a test daemon).
pub fn ensure_started() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    deploy::seed::clean_tmp();
    boot();
    let _ = std::thread::Builder::new().name("apps-supervisor".into()).spawn(|| loop {
        std::thread::sleep(Duration::from_millis(250));
        if deploy::enabled() {
            tick();
        }
    });
}

/// Open (or re-open) an App's front listener and keep the DB in step.
pub fn ensure_front(app: &AppRow) -> Result<u16, Fail> {
    let h = runtime().ok_or_else(|| fail(codes::ENGINE_UNAVAILABLE, 503, "the App front runtime couldn't start"))?.handle();
    let want = app.front_port.and_then(|p| u16::try_from(p).ok()).unwrap_or(0);
    let port = router::ensure_front(h, &app.app_id, want).map_err(|e| fail(codes::ENGINE_UNAVAILABLE, 503, e))?;
    if want != port {
        db(|c| {
            c.execute("UPDATE apps SET front_port = ?2 WHERE app_id = ?1", rusqlite::params![app.app_id, port as i64])?;
            c.execute("UPDATE published_services SET port = ?2 WHERE id = ?1", rusqlite::params![app.service_id, port as i64])
        })?;
        if want != 0 {
            k2_core::log_debug!("[apps] {}: front port {want} was taken; now {port} (repoint the URL)", app.app_id);
        }
    }
    Ok(port)
}

fn boot() {
    let t = timings();
    // Deploys a restart interrupted: stalled, and the agent hears (L3).
    if let Ok(rows) = db(store::unfinished_deployments) {
        for d in rows {
            let _ = db(|c| store::set_result(c, &d.id, store::RESULT_STALLED, Some("the K2 daemon restarted during this deploy")));
            stall_alert(&d, "the K2 daemon restarted during it");
        }
    }
    let apps = db(|c| store::list_apps(c, None)).unwrap_or_default();
    // Process-engine instances are adopted only while the owner has the
    // unwalled engine on; otherwise they are orphans (stopped below) and
    // their Apps are marked stopped, never restarted unwalled.
    let unwalled_ok = deploy::process_engine_enabled();
    let mut adopted = if unwalled_ok { engine().adopt().unwrap_or_default() } else { Vec::new() };
    let unwalled_orphans = if unwalled_ok { Vec::new() } else { engine().adopt().unwrap_or_default() };
    if let Some(g) = engine_for(Isolation::Container) {
        match g.adopt() {
            Ok(mut v) => adopted.append(&mut v),
            Err(e) => k2_core::log_debug!("[apps] guarded engine adopt: {}", e.message),
        }
        match super::process_exception::engine().adopt() {
            Ok(mut v) => adopted.append(&mut v),
            Err(e) => k2_core::log_debug!("[apps] process exception adopt: {}", e.message),
        }
    }
    let mut claimed = std::collections::BTreeSet::new();
    for app in &apps {
        let _ = ensure_front(app);
        let live = app
            .live_deployment
            .as_deref()
            .and_then(|d| adopted.iter().find(|h| h.app_id == app.app_id && h.deployment_id == d));
        if let (Some(h), Some(seed)) = (live, app.live_seed.as_deref()) {
            let inst = instance_for(h.clone(), seed);
            router::switch_with(&app.app_id, h.port, inst.client_ip);
            claimed.insert(h.deployment_id.clone());
            // Smoke r6: an unwalled instance outlives a restart (KillMode=
            // process). If its logs went through a pipe to the old daemon it
            // is relaunched now; otherwise it is checked every second for a
            // while. Walled engines adopt as before.
            let unwalled = is_process_engine(h);
            let stale = unwalled && super::process_engine::needs_relaunch(&h.deployment_id);
            with_rt(&app.app_id, |rt| {
                rt.live = Some(inst);
                if stale {
                    rt.relaunch = Some(WHY_STALE_SINK);
                } else if unwalled {
                    rt.adopt_check = Some(Instant::now() + Duration::from_secs(ADOPT_CHECK_SECS));
                }
            });
            audit(&app.app_id, "adopted", "system", json!({ "deployment": h.deployment_id, "port": h.port, "relaunch": stale }));
        } else if !unwalled_ok && runs_unwalled(app) && app.state != store::STATE_STOPPED {
            let _ = db(|c| store::set_state(c, &app.app_id, store::STATE_STOPPED, Some(UNWALLED_STOPPED)));
            audit(&app.app_id, "process_engine_off", "system", json!({ "stopped": true }));
        } else if boot_restarts(&app.state) && app.live_seed.is_some() {
            // Running (or waiting on a crash restart the old daemon had
            // scheduled when it went down: smoke r6 sat at `restarting` for
            // minutes) with no instance left: start it now, as the daemon's
            // restart, not a crash.
            with_rt(&app.app_id, |rt| {
                rt.next_restart_at = Some(now());
                rt.restart_why = Some(WHY_DAEMON_RESTARTED);
            });
        }
    }
    // Orphans (warm instances from before the restart, uninstalled Apps,
    // and everything on the process engine while it is off).
    let orphans: Vec<RunHandle> = adopted.into_iter().filter(|h| !claimed.contains(&h.deployment_id)).chain(unwalled_orphans).collect();
    for h in orphans {
        let grace = Duration::from_secs(t.stop_grace_secs);
        let _ = std::thread::Builder::new().name("app-orphan-stop".into()).spawn(move || {
            let _ = engine_of(&h).stop(&h, grace);
        });
    }
}

/// The App states a daemon boot restarts when no instance survived.
pub(crate) fn boot_restarts(state: &str) -> bool {
    state == store::STATE_RUNNING || state == STATE_RESTARTING
}

/// `crashed` waits on a restart in this state.
const STATE_RESTARTING: &str = "restarting";

fn instance_for(h: RunHandle, seed: &str) -> Instance {
    let m = seed_manifest(seed).ok();
    Instance {
        handle: h,
        seed: seed.to_string(),
        health_path: m.as_ref().map(|m| m.health_path()).unwrap_or_else(|| "/".into()),
        probe_as: if m.as_ref().is_some_and(|m| m.wants_test_guest_probe()) { ProbeAs::TestGuest } else { ProbeAs::Anonymous },
        probe_tcp: m.as_ref().is_some_and(|m| m.tcp_health()),
        drain_secs: m.as_ref().and_then(|m| m.host.drain_secs).unwrap_or(timings().drain_secs),
        client_ip: m.as_ref().map(|m| m.wants_client_ip()).unwrap_or(true),
    }
}

fn seed_manifest(hash: &str) -> Result<Manifest, Fail> {
    let row = db(|c| store::get_seed(c, hash))?.ok_or_else(|| fail(codes::SEED_NOT_FOUND, 404, format!("no Seed {hash}")))?;
    serde_json::from_value(row.manifest).map_err(|e| fail(codes::SEED_INVALID, 409, format!("stored manifest: {e}")))
}

// ── status ───────────────────────────────────────────────────────────

/// The live instance's port and seed (inspect).
pub fn live_instance(app_id: &str) -> Option<Instance> {
    state().lock().unwrap_or_else(|e| e.into_inner()).get(app_id).and_then(|rt| rt.live.clone())
}

/// `{live, warm, watchUntil, busy, restartAt, recentRestarts}` for inspect.
pub fn runtime_json(app_id: &str) -> serde_json::Value {
    let s = state().lock().unwrap_or_else(|e| e.into_inner());
    let Some(rt) = s.get(app_id) else { return json!({ "live": null, "warm": null, "busy": false }) };
    let inst = |i: &Option<Instance>| {
        i.as_ref().map(|i| json!({ "deployment": i.handle.deployment_id, "seed": i.seed, "port": i.handle.port, "pid": i.handle.pid }))
    };
    json!({
        "live": inst(&rt.live),
        "warm": inst(&rt.warm),
        "watchUntilSecs": rt.watch.as_ref().map(|w| w.until.saturating_duration_since(Instant::now()).as_secs()),
        "busy": rt.busy,
        "restartAt": rt.next_restart_at,
        "recentRestarts": rt.backoff.recent_restarts(),
        "engine": rt.live.as_ref().map(|i| engine_of(&i.handle).isolation().as_str()),
        "engineCaps": rt.live.as_ref().map(|i| engine_of(&i.handle).caps()),
    })
}

/// The Published drawer's status for an App's service row.
pub fn service_status(service_id: &str) -> Option<&'static str> {
    let app = db(|c| store::get_app_by_service(c, service_id)).ok().flatten()?;
    if router::live(&app.app_id).is_some() {
        return Some("running");
    }
    Some(match app.state.as_str() {
        store::STATE_CRASHLOOPING => "crashlooping",
        store::STATE_FAILED => "exited",
        "restarting" => "starting",
        _ => "stopped",
    })
}

// ── the pipeline ─────────────────────────────────────────────────────

/// Start a deploy (or rollback/start/restart) of `seed` and return its
/// deployment id at once; the pipeline runs on its own thread.
pub fn begin(app: &AppRow, seed: &str, kind: &str, actor: &str, reason: Option<&str>) -> Result<String, Fail> {
    if !with_rt(&app.app_id, |rt| {
        if rt.busy {
            false
        } else {
            rt.busy = true;
            true
        }
    }) {
        return Err(fail(codes::DEPLOY_IN_PROGRESS, 409, format!("{} has a deploy in progress; wait for it (k2 app history {})", app.app_id, app.app_id)));
    }
    let release = || with_rt(&app.app_id, |rt| rt.busy = false);
    let seed_row = match db(|c| store::get_seed(c, seed)) {
        Ok(Some(s)) => s,
        Ok(None) => {
            release();
            return Err(fail(codes::SEED_NOT_FOUND, 404, format!("no Seed {seed}")));
        }
        Err(e) => {
            release();
            return Err(e);
        }
    };
    // 0.46.3: a deploy checks the Seed's requires.sew / requires.voice
    // against this box's Sew (rollbacks and restarts don't: they keep or
    // put back what ran).
    if kind == store::KIND_DEPLOY {
        let req = serde_json::from_value::<Manifest>(seed_row.manifest.clone()).ok().and_then(|m| m.requires);
        if let Err((msg, _)) = super::requires::check_live(&app.app_id, req.as_ref()) {
            release();
            return Err(fail(codes::REQUIRES_UNMET, 409, msg));
        }
    }
    let commit = seed_row.source_ref.clone().unwrap_or_else(|| seed_row.hash.clone());
    let dep = match db(|c| {
        store::insert_deployment(
            c,
            &NewDeployment {
                app_id: &app.app_id,
                kind,
                from_seed: app.live_seed.as_deref(),
                to_seed: Some(seed),
                commit_ref: Some(&commit),
                actor,
                reason,
            },
        )
    }) {
        Ok(d) => d,
        Err(e) => {
            release();
            return Err(e);
        }
    };
    audit(&app.app_id, kind, actor, json!({ "deployment": dep, "from": app.live_seed, "to": seed, "commit": commit, "reason": reason }));
    with_rt(&app.app_id, |rt| rt.in_flight = Some(dep.clone()));
    let app_id = app.app_id.clone();
    let seed = seed.to_string();
    let dep2 = dep.clone();
    let kind = kind.to_string();
    let spawned = std::thread::Builder::new().name(format!("app-deploy-{app_id}")).spawn(move || {
        pipeline(&app_id, &dep2, &seed, &kind);
        with_rt(&app_id, |rt| {
            rt.busy = false;
            // A relaunch that never went live leaves the live one in place.
            rt.retire_on_live = false;
        });
        if kind == store::KIND_RESTART {
            restart_outcome(&app_id, &dep2);
        }
        // Last: the deploy's lock is gone before a waiter hears "done".
        with_rt(&app_id, |rt| {
            if rt.in_flight.as_deref() == Some(dep2.as_str()) {
                rt.in_flight = None;
            }
        });
    });
    if spawned.is_err() {
        with_rt(&app.app_id, |rt| rt.in_flight = None);
        release();
        let _ = db(|c| store::set_result(c, &dep, store::RESULT_FAILED, Some("could not start the deploy thread")));
        return Err(fail(codes::ENGINE_UNAVAILABLE, 503, "could not start the deploy thread"));
    }
    Ok(dep)
}

/// How a running (or starting) instance ended.
#[derive(Debug, Clone, Copy, Default)]
struct Exit {
    exited: bool,
    code: Option<i32>,
    signal: Option<i32>,
}

impl Exit {
    /// Read before the instance is stopped (the signal is only known while
    /// the engine still holds the child).
    fn of(h: &RunHandle) -> Exit {
        match engine_of(h).liveness(h) {
            Liveness::Exited(code) => Exit { exited: true, code, signal: super::process_engine::exit_signal(&h.deployment_id) },
            _ => Exit::default(),
        }
    }
}

fn pipeline_fail(app_id: &str, dep: &str, code: &str, msg: &str) {
    pipeline_fail_exit(app_id, dep, code, msg, Exit::default());
}

fn pipeline_fail_exit(app_id: &str, dep: &str, code: &str, msg: &str, exit: Exit) {
    let _ = db(|c| store::set_result(c, dep, store::RESULT_FAILED, Some(&format!("{code}: {msg}"))));
    audit(app_id, "failed", "system", json!({ "deployment": dep, "code": code, "error": msg }));
    let no_live = with_rt(app_id, |rt| rt.live.is_none());
    if no_live {
        let _ = db(|c| store::set_state(c, app_id, store::STATE_FAILED, Some(msg)));
    }
    // The failure history, and the App's agent hears (0.46.3). A crash
    // restart's own failure is a crash: `restart_outcome` records it.
    if code == codes::APP_NOT_FOUND {
        return;
    }
    if let Ok(Some(d)) = db(|c| store::get_deployment(c, dep)) {
        if d.kind == store::KIND_RESTART && d.actor == "system" {
            return;
        }
        super::failures::deploy_failed(&super::failures::DeployFail {
            app_id,
            deployment: dep,
            deploy_kind: &d.kind,
            actor: &d.actor,
            code,
            reason: msg,
            exit_code: exit.code,
            signal: exit.signal,
        });
    }
}

fn spec_for(app: &AppRow, m: &Manifest, dep: &str) -> AppSpec {
    let dir = deploy::app_dir(&app.app_id);
    let cfg = db(|c| k2_core::app_grace::config::load(c, &app.app_id)).unwrap_or_default();
    let open = app.path == "open";
    AppSpec {
        app_id: app.app_id.clone(),
        service_id: app.service_id.clone(),
        workspace_id: app.owner_workspace.clone(),
        deployment_id: dep.to_string(),
        isolation: Isolation::parse(&app.isolation).unwrap_or(Isolation::Process),
        argv: m.host.start.clone(),
        env: m.env.clone().into_iter().collect(),
        memory_limit_bytes: app.memory_limit_bytes,
        drain_secs: m.host.drain_secs.unwrap_or(timings().drain_secs),
        io_max_json: app.io_max_json.clone(),
        devices: app.devices(),
        // Read from the bindings at every start; labels the App took over
        // from its workspace's legacy services count too (0.46.3).
        public_origins: super::origins::with_taken_labels(super::origins::for_service(&app.service_id, &app.app_id), &super::labels::taken(&app.app_id)),
        data_dir: dir.join("data"),
        cache_dir: dir.join("cache"),
        home_dir: dir.join("home"),
        log_path: deploy::deployment_log_path(&app.app_id, dep),
        wall: k2_core::deploy::engine::WallSpec {
            open,
            // The relaxed container's own port and extra ports are open-path
            // features: a guarded App never gets them, whatever is stored.
            app_port: if open { cfg.app_port } else { None },
            extra_ports: if open { cfg.tcp_ports() } else { Vec::new() },
            tasks: cfg.resources.tasks,
            cpu_weight: cfg.resources.cpu_weight,
        },
    }
}

/// A free loopback port for a new instance, never in K2's reserved App
/// range (38600-38999: gateway links and the proxy; box prep also keeps it
/// out of the ephemeral range).
fn free_port() -> Result<u16, Fail> {
    for _ in 0..32 {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|e| fail(codes::ENGINE_UNAVAILABLE, 503, format!("no free port: {e}")))?;
        let p = l.local_addr().map_err(|e| fail(codes::ENGINE_UNAVAILABLE, 503, e.to_string()))?.port();
        if k2_root_proto::engine::app_port_ok(p) {
            return Ok(p);
        }
    }
    Err(fail(codes::ENGINE_UNAVAILABLE, 503, "no free port outside K2's reserved range"))
}

fn pipeline(app_id: &str, dep: &str, seed: &str, kind: &str) {
    let t = timings();
    let app = match db(|c| store::get_app(c, app_id)) {
        Ok(Some(a)) => a,
        _ => return pipeline_fail(app_id, dep, codes::APP_NOT_FOUND, "the App was removed"),
    };
    if let Err(e) = deploy::seed::verify(seed) {
        return pipeline_fail(app_id, dep, e.code, &e.message);
    }
    let m = match seed_manifest(seed) {
        Ok(m) => m,
        Err(e) => return pipeline_fail(app_id, dep, e.code, &e.message),
    };
    if !matches!(m.host.mode.as_str(), "cmd" | "next") {
        return pipeline_fail(
            app_id,
            dep,
            codes::STATIC_NOT_ON_RUNTIME,
            "static Apps are served by `k2 publish run --skin` in this release",
        );
    }
    let probe_as = if m.wants_test_guest_probe() { ProbeAs::TestGuest } else { ProbeAs::Anonymous };
    let headers = match health::probe_headers(app_id, probe_as) {
        Ok(h) => h,
        Err((code, msg)) => return pipeline_fail(app_id, dep, code, &msg),
    };
    let wanted = Isolation::parse(&app.isolation).unwrap_or(Isolation::Process);
    let Some(eng) = engine_for_app(&app) else {
        if runs_unwalled(&app) {
            return pipeline_fail(app_id, dep, codes::ENGINE_UNAVAILABLE, &format!("{app_id}: {}", deploy::PROCESS_ENGINE_OFF_HINT));
        }
        return pipeline_fail(
            app_id,
            dep,
            codes::ENGINE_UNAVAILABLE,
            &format!(
                "no engine on this box gives path={} isolation={} (the guarded engine and the process exception run on Linux boxes)",
                app.path,
                wanted.as_str()
            ),
        );
    };
    if let Err(e) = k2_core::deploy::engine::require_isolation(eng, wanted) {
        return pipeline_fail(app_id, dep, e.code, &e.message);
    }
    // K-M3: the App's seal key exists and is granted before the engine
    // reads the vault grants (every start: deploy, restart, rollback).
    super::seal::ensure_for_start(&app.app_id, &app.service_id);
    let spec = spec_for(&app, &m, dep);
    let seed_ref = SeedRef { hash: seed.to_string(), path: deploy::seed::seed_path(seed) };
    let prepared = match eng.prepare(&seed_ref, &spec) {
        Ok(p) => p,
        Err(e) => return pipeline_fail(app_id, dep, e.code, &e.message),
    };
    let port = match free_port() {
        Ok(p) => p,
        Err(e) => return pipeline_fail(app_id, dep, e.code, &e.message),
    };
    // An open-path App's extra ports sit on host loopback under their own
    // numbers: two instances can't hold them at once, so the running one
    // stops before the new one starts (no zero-downtime switch for an App
    // with extra ports; its front port answers 503 in between).
    if !spec.wall.extra_ports.is_empty() {
        let (live, warm) = with_rt(app_id, |rt| {
            rt.watch = None;
            (rt.live.take(), rt.warm.take())
        });
        for inst in [live, warm].into_iter().flatten() {
            let grace = Duration::from_secs(inst.drain_secs.min(t.stop_grace_secs.max(1)));
            if let Err(e) = engine_of(&inst.handle).stop(&inst.handle, grace) {
                k2_core::log_debug!("[apps] {app_id}: stopping {} before a restart with extra ports: {}", inst.handle.deployment_id, e.message);
            }
        }
    }
    let h = match eng.start(&spec, &prepared, port) {
        Ok(h) => h,
        Err(e) => return pipeline_fail(app_id, dep, e.code, &e.message),
    };
    let _ = db(|c| store::set_started(c, dep, port, h.pid.map(i64::from)));
    let path = m.health_path();
    let gate = Duration::from_secs(m.host.health.as_ref().and_then(|h| h.timeout_secs).unwrap_or(t.deploy_timeout_secs));
    let tcp = m.tcp_health();
    let healthy = health::wait_healthy_as(
        tcp,
        port,
        &path,
        &headers,
        Duration::from_millis(t.probe_every_ms),
        Duration::from_millis(t.probe_timeout_ms),
        gate,
        || !matches!(eng.liveness(&h), Liveness::Exited(_)),
    );
    if let Err(g) = healthy {
        let exit = Exit::of(&h);
        let _ = eng.stop(&h, Duration::from_secs(2));
        return pipeline_fail_exit(app_id, dep, g.code, &g.message, exit);
    }
    let inst = Instance {
        handle: h,
        seed: seed.to_string(),
        health_path: path,
        probe_as,
        probe_tcp: tcp,
        drain_secs: m.host.drain_secs.unwrap_or(t.drain_secs),
        client_ip: m.wants_client_ip(),
    };
    go_live(&app, dep, inst, kind, &t);
}

/// The switch: route new connections to `inst`, drain the old one, keep it
/// warm for the watch window.
fn go_live(app: &AppRow, dep: &str, inst: Instance, kind: &str, t: &Timings) {
    let (_, prev_be) = router::switch_with(&app.app_id, inst.handle.port, inst.client_ip);
    let watch_secs = t.watch_secs;
    let (older_warm, prev_live, replace) = with_rt(&app.app_id, |rt| {
        let older = rt.warm.take();
        let prev = rt.live.take();
        let replace = std::mem::take(&mut rt.retire_on_live);
        rt.live = Some(inst.clone());
        // A relaunch replaces the instance outright: nothing to roll back to.
        rt.warm = if replace { None } else { prev.clone() };
        rt.watch = Some(Watch { deployment_id: dep.to_string(), until: Instant::now() + Duration::from_secs(watch_secs) });
        rt.next_restart_at = None;
        rt.restart_why = None;
        rt.adopt_check = None;
        rt.live_fails = 0;
        rt.last_probe = None;
        (older, prev, replace)
    });
    let _ = db(|c| {
        store::set_went_live(c, dep, now() + watch_secs as i64)?;
        store::set_live(c, &app.app_id, Some(&inst.seed), Some(dep))?;
        store::set_state(c, &app.app_id, store::STATE_RUNNING, None)?;
        store::touch_seed(c, &inst.seed, now())
    });
    audit(
        &app.app_id,
        "live",
        "system",
        json!({ "deployment": dep, "kind": kind, "seed": inst.seed, "port": inst.handle.port, "warm": prev_live.as_ref().map(|p| p.handle.deployment_id.clone()) }),
    );
    super::surfaces::emit_deployed(&app.app_id);
    // Old connections drain in the background; the old instance stays up
    // (a relaunch stops it once they have drained).
    let drain = prev_live.as_ref().map(|p| p.drain_secs).unwrap_or(t.drain_secs);
    let replaced = if replace { prev_live.clone() } else { None };
    if prev_be.is_some() || replaced.is_some() {
        let grace = Duration::from_secs(t.stop_grace_secs);
        let app_id = app.app_id.clone();
        let _ = std::thread::Builder::new().name("app-drain".into()).spawn(move || {
            if let Some(be) = prev_be {
                router::drain_blocking(&be, Duration::from_secs(drain));
            }
            if let Some(old) = replaced {
                let _ = engine_of(&old.handle).stop(&old.handle, grace);
                let _ = db(|c| store::set_result(c, &old.handle.deployment_id, store::RESULT_SUPERSEDED, None));
                audit(&app_id, "stopped", "system", json!({ "deployment": old.handle.deployment_id, "result": store::RESULT_SUPERSEDED, "relaunch": true }));
            }
        });
    }
    // A deploy inside the watch window replaces the older warm one.
    if let Some(old) = older_warm {
        retire(&app.app_id, old, store::RESULT_SUPERSEDED, t);
    }
}

/// Stop an instance in the background and record its deployment's result.
fn retire(app_id: &str, inst: Instance, result: &'static str, t: &Timings) {
    let grace = Duration::from_secs(t.stop_grace_secs);
    let app_id = app_id.to_string();
    let _ = std::thread::Builder::new().name("app-retire".into()).spawn(move || {
        let _ = engine_of(&inst.handle).stop(&inst.handle, grace);
        let _ = db(|c| store::set_result(c, &inst.handle.deployment_id, result, None));
        audit(&app_id, "stopped", "system", json!({ "deployment": inst.handle.deployment_id, "result": result }));
    });
}

// ── the tick ─────────────────────────────────────────────────────────

fn probe(inst: &Instance, t: &Timings) -> bool {
    let headers = match health::probe_headers(&inst.handle.app_id, inst.probe_as) {
        Ok(h) => h,
        Err(_) => return false,
    };
    health::probe_as(inst.probe_tcp, inst.handle.port, &inst.health_path, &headers, Duration::from_millis(t.probe_timeout_ms)).healthy()
}

fn tick() {
    let t = timings();
    let apps: Vec<String> = state().lock().unwrap_or_else(|e| e.into_inner()).keys().cloned().collect();
    for app_id in apps {
        tick_app(&app_id, &t);
    }
    stall_check(&t);
}

fn tick_app(app_id: &str, t: &Timings) {
    let (busy, live, watch, warm, restart_at, last_probe) = with_rt(app_id, |rt| {
        (rt.busy, rt.live.clone(), rt.watch.clone(), rt.warm.clone(), rt.next_restart_at, rt.last_probe)
    });
    if busy {
        return;
    }
    let Some(live) = live else {
        if restart_at.is_some_and(|at| now() >= at) {
            let why = with_rt(app_id, |rt| {
                rt.next_restart_at = None;
                rt.restart_why.take()
            });
            if let Ok(Some(app)) = db(|c| store::get_app(c, app_id)) {
                if let Some(seed) = app.live_seed.clone() {
                    let reason = why.map(|w| format!("restart: {w}")).unwrap_or_else(|| "restart after a crash".to_string());
                    if let Err(e) = begin(&app, &seed, store::KIND_RESTART, "system", Some(&reason)) {
                        k2_core::log_debug!("[apps] {app_id}: restart failed to begin: {}", e.message);
                    }
                }
            }
        }
        return;
    };
    let exit = Exit::of(&live.handle);
    let exited = exit.exited;
    // A relaunch the boot asked for (an adopted instance whose logs went
    // through a pipe to the old daemon), or one the adopt check decided.
    if !exited {
        if let Some(why) = with_rt(app_id, |rt| rt.relaunch.take()) {
            return relaunch(app_id, &live, why);
        }
        if let Some(until) = with_rt(app_id, |rt| rt.adopt_check) {
            if last_probe.is_none_or(|p| p.elapsed() >= Duration::from_secs(1)) {
                let ok = probe(&live, t);
                let fails = with_rt(app_id, |rt| {
                    rt.last_probe = Some(Instant::now());
                    if ok {
                        rt.adopt_check = None;
                        rt.live_fails = 0;
                    } else {
                        rt.live_fails += 1;
                    }
                    rt.live_fails
                });
                if !ok && (fails >= FAILS_TO_UNHEALTHY || Instant::now() >= until) {
                    with_rt(app_id, |rt| {
                        rt.adopt_check = None;
                        rt.live_fails = 0;
                    });
                    return relaunch(app_id, &live, WHY_ADOPT_UNHEALTHY);
                }
            }
            return;
        }
    }
    // Health: every second in the watch window, every live_probe_secs after.
    let every = if watch.is_some() { Duration::from_secs(1) } else { Duration::from_secs(t.live_probe_secs.max(1)) };
    let due = t.live_probe_secs > 0 || watch.is_some();
    let mut unhealthy = false;
    if !exited && due && last_probe.is_none_or(|p| p.elapsed() >= every) {
        let ok = probe(&live, t);
        unhealthy = with_rt(app_id, |rt| {
            rt.last_probe = Some(Instant::now());
            if ok {
                rt.live_fails = 0;
                rt.backoff.on_healthy(now());
                false
            } else {
                rt.live_fails += 1;
                rt.live_fails >= FAILS_TO_UNHEALTHY
            }
        });
    }
    if exited || unhealthy {
        let why = if exited { "exited" } else { "failed its health checks" };
        if let Some(w) = watch {
            if let Some(prev) = warm.filter(|p| !matches!(engine_of(&p.handle).liveness(&p.handle), Liveness::Exited(_))) {
                return auto_rollback(app_id, &w, live, prev, why, exit, t);
            }
        }
        return crashed(app_id, live, why, exit, t);
    }
    if let Some(w) = watch {
        if Instant::now() >= w.until {
            let prev = with_rt(app_id, |rt| {
                rt.watch = None;
                rt.warm.take()
            });
            audit(app_id, "watch_passed", "system", json!({ "deployment": w.deployment_id }));
            if let Some(p) = prev {
                retire(app_id, p, store::RESULT_SUPERSEDED, t);
            }
        }
    }
}

/// Start a fresh instance of the live Seed beside `live` (still routed until
/// the new one passes its gate, then stopped). The daemon's restart, not
/// the App's crash: no backoff, no crash record, no agent woken. A fresh
/// instance that fails its gate leaves `live` as it is (the normal health
/// checks then decide).
fn relaunch(app_id: &str, live: &Instance, why: &'static str) {
    let Ok(Some(app)) = db(|c| store::get_app(c, app_id)) else { return };
    with_rt(app_id, |rt| rt.retire_on_live = true);
    if let Err(e) = begin(&app, &live.seed, store::KIND_RESTART, "system", Some(why)) {
        with_rt(app_id, |rt| rt.retire_on_live = false);
        k2_core::log_debug!("[apps] {app_id}: relaunch failed to begin: {}", e.message);
    } else {
        audit(app_id, "relaunch", "system", json!({ "deployment": live.handle.deployment_id, "why": why }));
    }
}

fn auto_rollback(app_id: &str, w: &Watch, bad: Instance, prev: Instance, why: &str, exit: Exit, t: &Timings) {
    let (_, bad_be) = router::switch_with(app_id, prev.handle.port, prev.client_ip);
    with_rt(app_id, |rt| {
        rt.live = Some(prev.clone());
        rt.warm = None;
        rt.watch = None;
        rt.live_fails = 0;
        rt.last_probe = None;
    });
    let msg = format!("{why} inside the watch window; rolled back to {}", &prev.seed[..12.min(prev.seed.len())]);
    let row = db(|c| {
        store::set_result(c, &w.deployment_id, store::RESULT_ROLLED_BACK, Some(&msg))?;
        let id = store::insert_deployment(
            c,
            &NewDeployment {
                app_id,
                kind: store::KIND_AUTO_ROLLBACK,
                from_seed: Some(&bad.seed),
                to_seed: Some(&prev.seed),
                commit_ref: None,
                actor: "system",
                reason: Some(&msg),
            },
        )?;
        store::set_went_live(c, &id, now())?;
        store::set_live(c, app_id, Some(&prev.seed), Some(&prev.handle.deployment_id))?;
        store::set_state(c, app_id, store::STATE_RUNNING, None)?;
        Ok(id)
    });
    audit(app_id, "auto_rollback", "system", json!({ "deployment": row.as_ref().ok(), "from": bad.seed, "to": prev.seed, "why": why }));
    super::failures::rolled_back(app_id, &w.deployment_id, exit.exited, exit.code, exit.signal);
    super::surfaces::emit_deployed(app_id);
    if let Some(be) = bad_be {
        let d = bad.drain_secs;
        let _ = std::thread::Builder::new().name("app-drain".into()).spawn(move || {
            router::drain_blocking(&be, Duration::from_secs(d));
        });
    }
    let grace = Duration::from_secs(t.stop_grace_secs);
    let _ = std::thread::Builder::new().name("app-rollback-stop".into()).spawn(move || {
        let _ = engine_of(&bad.handle).stop(&bad.handle, grace);
    });
    if let Ok(Some(app)) = db(|c| store::get_app(c, app_id)) {
        super::alerts::notify(
            &app.owner_workspace,
            &format!(
                "[k2 apps] App {app_id}: the new deploy {why} inside its watch window, so K2 rolled it back to the previous Seed. \
                 Read why: k2 app errors {app_id} ; history: k2 app history {app_id}"
            ),
        );
    }
}

fn crashed(app_id: &str, inst: Instance, why: &str, exit: Exit, t: &Timings) {
    router::clear(app_id);
    let (decision, restarts) = with_rt(app_id, |rt| {
        rt.live = None;
        rt.watch = None;
        rt.live_fails = 0;
        rt.last_probe = None;
        let d = rt.backoff.on_crash(now());
        if let Decision::Restart { after_secs } = d {
            rt.next_restart_at = Some(now() + after_secs);
        }
        (d, rt.backoff.recent_restarts())
    });
    // The failure history; the first crash in the breaker window wakes the
    // App's agent, the rest fold into the crash-loop alert (0.46.3).
    super::failures::crash(&super::failures::Crash {
        app_id,
        deployment: &inst.handle.deployment_id,
        exited: exit.exited,
        exit_code: exit.code,
        signal: exit.signal,
        restart_count: restarts as u32,
        restart_in_secs: match decision {
            Decision::Restart { after_secs } => Some(after_secs),
            Decision::Breaker => None,
        },
        what: None,
    });
    let grace = Duration::from_secs(t.stop_grace_secs.min(5));
    let h = inst.handle.clone();
    let _ = std::thread::Builder::new().name("app-crash-stop".into()).spawn(move || {
        let _ = engine_of(&h).stop(&h, grace);
    });
    let _ = db(|c| store::set_result(c, &inst.handle.deployment_id, store::RESULT_EXITED, Some(why)));
    match decision {
        Decision::Restart { after_secs } => {
            let _ = db(|c| store::set_state(c, app_id, STATE_RESTARTING, Some(why)));
            audit(app_id, "crashed", "system", json!({ "deployment": inst.handle.deployment_id, "why": why, "restartInSecs": after_secs }));
        }
        Decision::Breaker => breaker(app_id, why),
    }
}

/// A crash restart that failed its gate counts as another crash (backoff
/// and the breaker apply), so a broken Seed can't restart forever.
fn restart_outcome(app_id: &str, dep: &str) {
    let failed = db(|c| store::get_deployment(c, dep)).ok().flatten().is_some_and(|d| d.result == store::RESULT_FAILED);
    if !failed || with_rt(app_id, |rt| rt.live.is_some()) {
        return;
    }
    let (decision, restarts) = with_rt(app_id, |rt| {
        let d = rt.backoff.on_crash(now());
        if let Decision::Restart { after_secs } = d {
            rt.next_restart_at = Some(now() + after_secs);
        }
        (d, rt.backoff.recent_restarts())
    });
    super::failures::crash(&super::failures::Crash {
        app_id,
        deployment: dep,
        restart_count: restarts as u32,
        restart_in_secs: match decision {
            Decision::Restart { after_secs } => Some(after_secs),
            Decision::Breaker => None,
        },
        what: Some("failed the health gate when K2 restarted it"),
        ..Default::default()
    });
    match decision {
        Decision::Restart { .. } => {
            let _ = db(|c| store::set_state(c, app_id, "restarting", Some("the restart failed its health gate")));
        }
        Decision::Breaker => breaker(app_id, "its restarts keep failing the health gate"),
    }
}

fn breaker(app_id: &str, why: &str) {
    let msg = format!("{why}; restarted 6 times within an hour, so K2 stopped restarting it");
    let _ = db(|c| store::set_state(c, app_id, store::STATE_CRASHLOOPING, Some(&msg)));
    audit(app_id, "crashlooping", "system", json!({ "why": why }));
    if let Ok(Some(app)) = db(|c| store::get_app(c, app_id)) {
        super::alerts::notify(
            &app.owner_workspace,
            &format!(
                "[k2 apps] App {app_id} is crash-looping ({why}) and K2 stopped restarting it. \
                 Read why: k2 app errors {app_id} --kind crash ; fix and deploy, or k2 app start {app_id}"
            ),
        );
    }
}

fn stall_alert(d: &DeploymentRow, why: &str) {
    audit(&d.app_id, "stalled", "system", json!({ "deployment": d.id, "why": why }));
    if let Ok(Some(app)) = db(|c| store::get_app(c, &d.app_id)) {
        super::alerts::notify(
            &app.owner_workspace,
            &format!(
                "[k2 apps] App {}: deploy {} by {} (commit {}) stalled: {why}. It is not live. \
                 Check: k2 app history {} ; logs: k2 app logs {} --deployment {}",
                d.app_id,
                d.id,
                d.actor,
                d.commit_ref.as_deref().unwrap_or("-"),
                d.app_id,
                d.app_id,
                d.id
            ),
        );
    }
}

fn stall_check(t: &Timings) {
    let Ok(rows) = db(store::unfinished_deployments) else { return };
    for d in rows {
        if now() - d.started_at >= t.stall_secs as i64 {
            let _ = db(|c| store::set_result(c, &d.id, store::RESULT_STALLED, Some(&format!("not live after {}s", t.stall_secs))));
            stall_alert(&d, &format!("not live after {} minutes", t.stall_secs / 60));
        }
    }
}

// ── verbs ────────────────────────────────────────────────────────────

/// The Seed a rollback goes to: `to`, else the newest earlier Seed that
/// went live.
pub fn rollback_target(app: &AppRow, to: Option<&str>) -> Result<String, Fail> {
    if let Some(t) = to {
        let row = db(|c| store::find_seed(c, t))?.ok_or_else(|| fail(codes::SEED_NOT_FOUND, 404, format!("no Seed '{t}'")))?;
        return Ok(row.hash);
    }
    let live = app.live_seed.clone();
    let deps = db(|c| store::list_deployments(c, &app.app_id, 50))?;
    deps.iter()
        .filter(|d| d.live_at.is_some())
        .filter_map(|d| d.to_seed.clone())
        .find(|s| Some(s) != live.as_ref())
        .ok_or_else(|| fail(codes::NOTHING_TO_ROLL_BACK, 409, format!("{} has no earlier Seed that went live", app.app_id)))
}

/// Refuse a rollback across an applied data migration (§7.3) unless the
/// newer Seed declares a down step.
pub fn check_data_revision(app: &AppRow, target: &str) -> Result<(), Fail> {
    let Some(live) = app.live_seed.as_deref() else { return Ok(()) };
    let cur = db(|c| store::get_seed(c, live))?;
    let tgt = db(|c| store::get_seed(c, target))?;
    let (Some(cur), Some(tgt)) = (cur, tgt) else { return Ok(()) };
    if cur.data_revision > tgt.data_revision {
        let down = cur.manifest.get("dataDown").and_then(|v| v.as_bool()).unwrap_or(false);
        if !down {
            return Err(fail(
                codes::ROLLBACK_CROSSES_MIGRATION,
                409,
                format!(
                    "the live Seed is at data revision {} and the target at {}; code rollback isn't data rollback. \
                     Deploy a Seed that declares dataDown, or roll the data back first",
                    cur.data_revision, tgt.data_revision
                ),
            ));
        }
    }
    Ok(())
}

/// Instant rollback to the warm instance when it holds `target`.
/// `Ok(Some(deployment))` when it switched; `Ok(None)` when there is no
/// warm instance for that Seed (the caller runs the pipeline).
pub fn rollback_to_warm(app: &AppRow, target: &str, actor: &str) -> Result<Option<String>, Fail> {
    let t = timings();
    let warm = with_rt(&app.app_id, |rt| {
        if rt.busy {
            return Err(fail(codes::DEPLOY_IN_PROGRESS, 409, "a deploy is in progress"));
        }
        Ok(rt.warm.clone().filter(|w| w.seed == target))
    })?;
    let Some(warm) = warm else { return Ok(None) };
    if matches!(engine_of(&warm.handle).liveness(&warm.handle), Liveness::Exited(_)) || !probe(&warm, &t) {
        return Ok(None);
    }
    let (_, bad_be) = router::switch_with(&app.app_id, warm.handle.port, warm.client_ip);
    let bad = with_rt(&app.app_id, |rt| {
        let bad = rt.live.take();
        rt.live = Some(warm.clone());
        rt.warm = None;
        rt.watch = None;
        bad
    });
    let id = db(|c| {
        let id = store::insert_deployment(
            c,
            &NewDeployment {
                app_id: &app.app_id,
                kind: store::KIND_ROLLBACK,
                from_seed: app.live_seed.as_deref(),
                to_seed: Some(target),
                commit_ref: None,
                actor,
                reason: Some("rollback to the warm previous instance"),
            },
        )?;
        store::set_went_live(c, &id, now())?;
        if let Some(b) = &bad {
            store::set_result(c, &b.handle.deployment_id, store::RESULT_ROLLED_BACK, Some("rolled back"))?;
        }
        store::set_live(c, &app.app_id, Some(target), Some(&warm.handle.deployment_id))?;
        store::set_state(c, &app.app_id, store::STATE_RUNNING, None)?;
        Ok(id)
    })?;
    audit(&app.app_id, "rollback", actor, json!({ "deployment": id, "to": target, "instant": true }));
    super::surfaces::emit_deployed(&app.app_id);
    if let Some(be) = bad_be {
        let d = bad.as_ref().map(|b| b.drain_secs).unwrap_or(t.drain_secs);
        let _ = std::thread::Builder::new().name("app-drain".into()).spawn(move || {
            router::drain_blocking(&be, Duration::from_secs(d));
        });
    }
    if let Some(b) = bad {
        retire(&app.app_id, b, store::RESULT_ROLLED_BACK, &t);
    }
    Ok(Some(id))
}

/// Stop every instance and the route (blocking; drains first).
pub fn stop_app(app: &AppRow, actor: &str) -> Result<(), Fail> {
    let t = timings();
    let (live, warm) = with_rt(&app.app_id, |rt| {
        if rt.busy {
            return Err(fail(codes::DEPLOY_IN_PROGRESS, 409, "a deploy is in progress; stop after it ends"));
        }
        rt.watch = None;
        rt.next_restart_at = None;
        Ok((rt.live.take(), rt.warm.take()))
    })?;
    if let Some(be) = router::clear(&app.app_id) {
        let d = live.as_ref().map(|l| l.drain_secs).unwrap_or(t.drain_secs);
        router::drain_blocking(&be, Duration::from_secs(d));
    }
    for inst in [live, warm].into_iter().flatten() {
        let _ = engine_of(&inst.handle).stop(&inst.handle, Duration::from_secs(t.stop_grace_secs));
        let _ = db(|c| store::set_result(c, &inst.handle.deployment_id, store::RESULT_STOPPED, None));
    }
    db(|c| store::set_state(c, &app.app_id, store::STATE_STOPPED, None))?;
    // S14: a stopped App has no gateway link and no live pass.
    let revoked = super::gateway::close(&app.app_id, Some(&app.service_id), true);
    audit(&app.app_id, "stop", actor, json!({ "passesRevoked": revoked }));
    Ok(())
}

/// Clear the crash-loop state (an owner `start` after `crashlooping`).
pub fn reset_backoff(app_id: &str) {
    with_rt(app_id, |rt| rt.backoff.reset());
    super::failures::clear_crash(app_id);
}

/// Forget an App's runtime state (uninstall; after `stop_app`).
pub fn forget(app_id: &str) {
    state().lock().unwrap_or_else(|e| e.into_inner()).remove(app_id);
    router::clear(app_id);
    router::close_front(app_id);
    super::gateway::close(app_id, None, false);
}

/// Wait (bounded) for a deployment to leave pending/starting.
pub fn wait_done(id: &str, secs: u64) -> Option<DeploymentRow> {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        let row = db(|c| store::get_deployment(c, id)).ok().flatten()?;
        // Done = a final result AND its pipeline thread has let go of the
        // App. The row goes live inside the pipeline, before `busy` clears:
        // a deploy (or start/stop) right after the answer used to meet a
        // false 409 deploy_in_progress (s7 flake, Linux).
        let finished = !matches!(row.result.as_str(), store::RESULT_PENDING | store::RESULT_STARTING)
            && !with_rt(&row.app_id, |rt| rt.in_flight.as_deref() == Some(id));
        if finished || Instant::now() >= end {
            return Some(row);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Shared for tests: the backend live for an App right now.
pub fn live_port(app_id: &str) -> Option<u16> {
    router::live(app_id).map(|b| b.port)
}
