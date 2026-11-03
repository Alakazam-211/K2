//! prd-app-runtime-update-continuity-v1 S1 through the real pipeline, with
//! a test engine standing in for `k2-app-engine` (its `prepare` can refuse
//! as K2's own engine would, `start` serves a real loopback port) and a fake
//! boot env standing in for the engine socket's adopt.
//!
//! Each test owns its App ids (`test_hooks::install`): the process's own
//! supervisor tick and boot leave them alone, and the test drives
//! `boot_with` / `tick_with` itself. The pipeline runs on its own threads,
//! so these tests use the shared test DB (never a thread-scoped one) and a
//! temp HOME for Seeds and the failure history.

use std::cell::Cell;
use std::collections::HashSet;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};

use k2_core::deploy::engine::{AppSpec, CgroupStats, EngineCaps, EngineError, ExecStream, Prepared, SeedRef};
use rusqlite::params;

use super::*;
use crate::apps::runtime_notes::{captured, Captured};

const OK: u8 = 0;
const PROTOCOL_2: u8 = 1;
const NO_SOCKET: u8 = 2;
/// The App itself is broken: it starts but never answers its health check.
const APP_BROKEN: u8 = 3;

struct TestEngine {
    mode: AtomicU8,
    prepares: AtomicU32,
    starts: AtomicU32,
    stopped: Mutex<HashSet<String>>,
    /// What each start could see of the App's box services, read as the real
    /// engine reads them: forwards, `K2_BOX_*`, the ports the nft rule opens.
    box_at_start: Mutex<Vec<BoxAtStart>>,
}

#[derive(Debug, Clone)]
struct BoxAtStart {
    forwards: Vec<(u16, u16)>,
    env: Vec<(String, String)>,
    door: Vec<u16>,
}

impl TestEngine {
    fn leak(mode: u8) -> &'static TestEngine {
        Box::leak(Box::new(TestEngine { mode: AtomicU8::new(mode), prepares: AtomicU32::new(0), starts: AtomicU32::new(0), stopped: Mutex::new(HashSet::new()), box_at_start: Mutex::new(Vec::new()) }))
    }
    fn set(&self, mode: u8) {
        self.mode.store(mode, Ordering::SeqCst);
    }
}

/// A loopback HTTP server answering 200 to everything (the App), on the
/// listener the pipeline's `free_port` kept bound for it.
fn serve(port: u16) -> std::io::Result<()> {
    let l = match super::test_hooks::take_held_port(port) {
        Some(l) => l,
        None => std::net::TcpListener::bind(("127.0.0.1", port))?,
    };
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            std::thread::spawn(move || {
                let mut b = [0u8; 2048];
                let _ = s.read(&mut b);
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            });
        }
    });
    Ok(())
}

impl RuntimeEngine for TestEngine {
    fn isolation(&self) -> Isolation {
        Isolation::Container
    }
    fn caps(&self) -> EngineCaps {
        EngineCaps::default()
    }
    fn prepare(&self, s: &SeedRef, spec: &AppSpec) -> k2_core::deploy::engine::Result<Prepared> {
        self.prepares.fetch_add(1, Ordering::SeqCst);
        match self.mode.load(Ordering::SeqCst) {
            PROTOCOL_2 => Err(EngineError::new(
                codes::ENGINE_PROTOCOL_MISMATCH,
                "the App engine speaks protocol 2 and this daemon 3: k2 app engine install",
            )),
            NO_SOCKET => Err(EngineError::new(
                codes::ENGINE_NOT_ANSWERING,
                "the App engine isn't answering at /run/k2app/engine.sock (No such file or directory)",
            )),
            _ => {
                // As the guarded engine does (R10): the version prepare checked.
                super::super::guarded::remember_checked(&spec.deployment_id, serde_json::json!({ "version": "9.9.9-test", "protocol": 3 }));
                Ok(Prepared { workdir: s.path.clone() })
            }
        }
    }
    fn start(&self, spec: &AppSpec, _: &Prepared, port: u16) -> k2_core::deploy::engine::Result<RunHandle> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        let app = spec.app_id.as_str();
        self.box_at_start.lock().unwrap().push(BoxAtStart {
            forwards: crate::apps::box_service_routes::forwards_for(app, port).0.iter().map(|f| (f.app_port, f.host_port)).collect(),
            env: crate::apps::box_service_routes::env_for(app, false).into_iter().collect(),
            door: crate::apps::box_service_routes::allowed_ports(app),
        });
        if self.mode.load(Ordering::SeqCst) != APP_BROKEN {
            serve(port).map_err(|e| EngineError::new(codes::ENGINE_UNAVAILABLE, e.to_string()))?;
        } else {
            // Broken: nothing answers on the port (the kept listener goes).
            drop(super::test_hooks::take_held_port(port));
        }
        Ok(RunHandle { app_id: spec.app_id.clone(), deployment_id: spec.deployment_id.clone(), port, pid: None, engine_ref: "test:".into() })
    }
    fn liveness(&self, h: &RunHandle) -> Liveness {
        if self.stopped.lock().unwrap().contains(&h.deployment_id) {
            Liveness::Exited(Some(0))
        } else {
            Liveness::Running
        }
    }
    fn stop(&self, h: &RunHandle, _: Duration) -> k2_core::deploy::engine::Result<()> {
        self.stopped.lock().unwrap().insert(h.deployment_id.clone());
        Ok(())
    }
    fn exec(&self, _: &RunHandle, _: &[String], _: bool) -> k2_core::deploy::engine::Result<ExecStream> {
        Err(EngineError::new("unsupported", "no exec"))
    }
    fn stats(&self, _: &RunHandle) -> k2_core::deploy::engine::Result<CgroupStats> {
        Ok(CgroupStats::default())
    }
    fn adopt(&self) -> k2_core::deploy::engine::Result<Vec<RunHandle>> {
        Ok(vec![])
    }
}

/// The engine socket's adopt at boot: refuses `fails` times, then answers.
struct FakeBoot<'a> {
    fails: Cell<u32>,
    answer: Vec<RunHandle>,
    clock: Cell<Duration>,
    reported: Cell<u32>,
    protocol_checks: Cell<u32>,
    /// Runs on every refused adopt (assert nothing started meanwhile).
    during: &'a dyn Fn(),
}

impl super::super::engine_gate::WaitOps for FakeBoot<'_> {
    fn adopt(&self) -> Result<Vec<RunHandle>, String> {
        if self.fails.get() > 0 {
            self.fails.set(self.fails.get() - 1);
            (self.during)();
            return Err("engine_caller_refused: App engine: caller_not_daemon".into());
        }
        Ok(self.answer.clone())
    }
    fn elapsed(&self) -> Duration {
        self.clock.get()
    }
    fn sleep(&self, d: Duration) {
        self.clock.set(self.clock.get() + d);
    }
    fn not_answering(&self, _waited: Duration, _e: &str) {
        self.reported.set(self.reported.get() + 1);
    }
}

impl BootEnv for FakeBoot<'_> {
    fn check_protocol(&self) {
        self.protocol_checks.set(self.protocol_checks.get() + 1);
    }
}

fn boot_env<'a>(fails: u32, answer: Vec<RunHandle>, during: &'a dyn Fn()) -> FakeBoot<'a> {
    FakeBoot { fails: Cell::new(fails), answer, clock: Cell::new(Duration::ZERO), reported: Cell::new(0), protocol_checks: Cell::new(0), during }
}

fn answering(answer: Vec<RunHandle>) -> FakeBoot<'static> {
    boot_env(0, answer, &|| {})
}

struct Fx {
    _home: crate::test_support::TempHome,
    seed: String,
}

/// A temp HOME, a stored Seed whose health gate is 2 s, and no agent wakes.
fn fx() -> Fx {
    let home = crate::test_support::TempHome::new();
    super::super::alerts::set_sink(Some(Box::new(|_: &str, _: &str| {})));
    let f = home.path().join("src").join("app");
    std::fs::create_dir_all(&f).unwrap();
    std::fs::write(f.join("server.js"), "require('http').createServer((q,s)=>s.end('ok')).listen(process.env.PORT)").unwrap();
    let manifest = serde_json::json!({
        "schema": 1, "kind": "app", "id": "s1", "name": "s1", "version": "1.0.0",
        "host": { "mode": "cmd", "start": ["node", "server.js"], "health": { "path": "/", "timeoutSecs": 2 } }
    });
    std::fs::write(f.join("manifest.json"), manifest.to_string()).unwrap();
    let st = k2_core::deploy::seed::store_folder(&f, &std::collections::BTreeSet::new()).unwrap();
    let h = k2_core::db::shared();
    let c = h.lock();
    store::upsert_seed(
        &c,
        &store::SeedRow {
            hash: st.hash.clone(),
            seed_id: "s1".into(),
            version: Some("1.0.0".into()),
            host_mode: "cmd".into(),
            manifest,
            created_with: serde_json::json!({}),
            report: serde_json::json!({}),
            signer: "owner".into(),
            origin: "build".into(),
            built_for: None,
            built_from: None,
            source_ref: None,
            data_revision: 0,
            size_bytes: st.size_bytes as i64,
            created_at: 1,
            last_used_at: 1,
        },
    )
    .unwrap();
    Fx { _home: home, seed: st.hash }
}

/// An installed App on `eng` with `seed` live in the DB and `state`.
fn app(id: &str, seed: &str, state: &str, eng: &'static TestEngine) {
    test_hooks::install(id, eng);
    let h = k2_core::db::shared();
    let c = h.lock();
    c.execute("DELETE FROM deployments WHERE app_id = ?1", params![id]).unwrap();
    c.execute("DELETE FROM apps WHERE app_id = ?1", params![id]).unwrap();
    store::insert_app(
        &c,
        &store::NewApp {
            app_id: id,
            service_id: &format!("svc-{id}"),
            seed_id: Some("s1"),
            owner_workspace: &format!("ws-{id}"),
            isolation: "container",
            memory_limit_bytes: None,
            front_port: 0,
            created_by: "owner",
        },
    )
    .unwrap();
    store::set_live(&c, id, Some(seed), Some(&format!("d-old-{id}"))).unwrap();
    store::set_state(&c, id, state, None).unwrap();
}

fn app_row(id: &str) -> AppRow {
    db(|c| store::get_app(c, id)).unwrap().unwrap()
}

fn deployments(id: &str) -> Vec<DeploymentRow> {
    db(|c| store::list_deployments(c, id, 100)).unwrap()
}

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Tick `only` until `done` holds (or fail loudly after `secs`). `done` is
/// checked before each tick, so a restart due "now" never runs ahead.
fn tick_until(only: &[String], seen: &AtomicU64, secs: u64, what: &str, done: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        if done() {
            return;
        }
        assert!(Instant::now() < end, "timed out waiting for: {what}");
        // Tick only once every pipeline has recorded its outcome (a restart
        // due "now" would otherwise begin while the last one still logs).
        if only.iter().all(|a| idle(a)) {
            tick_with(seen, Some(only));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// No pipeline thread of `id` is running (its outcome is recorded).
fn idle(id: &str) -> bool {
    with_rt(id, |rt| !rt.busy && rt.in_flight.is_none())
}

/// Wait for every pipeline thread of `id` to finish.
fn settle(id: &str) {
    let end = Instant::now() + Duration::from_secs(30);
    while with_rt(id, |rt| rt.busy || rt.in_flight.is_some()) {
        assert!(Instant::now() < end, "{id}: its deploy thread never finished");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Make a pending restart due now (stands in for the clock passing).
fn due_now(id: &str) {
    with_rt(id, |rt| {
        assert!(rt.next_restart_at.is_some(), "{id}: a restart is scheduled");
        rt.next_restart_at = Some(now());
    });
}

fn lines_for(id: &str) -> Vec<String> {
    captured()
        .into_iter()
        .filter_map(|c| match c {
            Captured::Line(l) if l.contains(&format!(" {id} ")) => Some(l),
            _ => None,
        })
        .collect()
}

fn notices_for(id: &str) -> Vec<String> {
    captured()
        .into_iter()
        .filter_map(|c| match c {
            Captured::Notice { title, .. } if title.contains(&format!("App {id} ")) => Some(title),
            _ => None,
        })
        .collect()
}

fn live(id: &str) -> bool {
    with_rt(id, |rt| rt.live.is_some()) && app_row(id).state == store::STATE_RUNNING
}

/// T27 (A3/R33): the engine refuses adopt (caller_not_daemon, a stale
/// socket, an engine mid-restart) for a while, then answers with the
/// surviving instance: zero new deployments, the survivor is routed, and
/// nothing starts while it waits; past 120 s one not-answering report and
/// still no start.
#[test]
fn boot_waits_for_adopt_and_adopts_the_survivor_without_a_second_start() {
    let f = fx();
    let eng = TestEngine::leak(OK);
    let id = "s1t-adopt";
    app(id, &f.seed, store::STATE_RUNNING, eng);
    let survivor_port = {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        l.local_addr().unwrap().port()
    };
    serve(survivor_port).unwrap();
    let survivor = RunHandle { app_id: id.into(), deployment_id: format!("d-old-{id}"), port: survivor_port, pid: Some(4242), engine_ref: "test:".into() };
    let only = ids(&[id]);
    let nothing_started = || {
        assert!(deployments(id).is_empty(), "no deployment may start before adopt answers");
        assert_eq!(eng.starts.load(Ordering::SeqCst), 0);
    };
    // 10 s of refusals (5 × 2 s), then the answer.
    let env = boot_env(5, vec![survivor.clone()], &nothing_started);
    boot_with(&env, false, Vec::new(), now(), Some(&only));
    assert_eq!((env.clock.get(), env.reported.get(), env.protocol_checks.get()), (Duration::from_secs(10), 0, 1));
    assert_eq!(with_rt(id, |rt| rt.live.as_ref().map(|i| i.handle.clone())), Some(survivor.clone()), "the survivor is the live instance");
    assert_eq!(router::live(id).map(|b| b.port), Some(survivor_port), "and routed");
    let seen = AtomicU64::new(super::super::engine_gate::generation());
    for _ in 0..5 {
        tick_with(&seen, Some(&only));
    }
    assert!(deployments(id).is_empty(), "no second instance, ever");
    assert_eq!(eng.starts.load(Ordering::SeqCst), 0);

    // Past 120 s without an answer: one report, still waiting, no start.
    let id2 = "s1t-adopt-late";
    app(id2, &f.seed, store::STATE_RUNNING, eng);
    let none2 = || assert!(deployments(id2).is_empty());
    let env = boot_env(70, vec![], &none2);
    boot_with(&env, false, Vec::new(), now(), Some(&ids(&[id2])));
    assert_eq!((env.reported.get(), env.clock.get()), (1, Duration::from_secs(140)), "one report at 120 s; it kept waiting");
    // It answered with nothing: now (and only now) the restart is scheduled.
    assert!(with_rt(id2, |rt| rt.next_restart_at.is_some()));
}

/// T25 (the 10-23 replay, both causes) and T31 (A4): H1, the boot race
/// (no socket yet), and H3, a protocol-2 engine. Each restart fails
/// K2-side: `waiting_engine`, never a crash (no backoff step, no
/// breaker even after many tries), an ERROR line per attempt, one owner
/// notice. The heal's install lands → both are live within the next
/// tick, with no CLI call. An owner-stopped App stays stopped.
#[test]
fn the_10_23_replay_waits_for_the_engine_and_comes_back_on_its_own() {
    let f = fx();
    let h1 = TestEngine::leak(NO_SOCKET);
    let h3 = TestEngine::leak(PROTOCOL_2);
    let (a1, a3, stopped) = ("s1t-h1", "s1t-h3", "s1t-stopped");
    app(a1, &f.seed, store::STATE_RUNNING, h1);
    app(a3, &f.seed, store::STATE_RUNNING, h3);
    app(stopped, &f.seed, store::STATE_STOPPED, h3);
    let only = ids(&[a1, a3, stopped]);
    boot_with(&answering(vec![]), false, Vec::new(), now(), Some(&only));
    let seen = AtomicU64::new(super::super::engine_gate::generation());
    // 12 attempts each (an hour of "every 60 s" would be 60; the breaker
    // trips at 6, so 12 proves it never counts).
    for round in 0..12 {
        for a in [a1, a3] {
            if round > 0 {
                due_now(a);
            }
        }
        tick_until(&only, &seen, 20, "both restarts failed K2-side", || {
            [a1, a3].iter().all(|a| deployments(a).len() == round + 1 && idle(a))
        });
        settle(a1);
        settle(a3);
    }
    for (a, code) in [(a1, codes::ENGINE_NOT_ANSWERING), (a3, codes::ENGINE_PROTOCOL_MISMATCH)] {
        let row = app_row(a);
        assert_eq!(row.state, store::STATE_WAITING_ENGINE, "{a}: {row:?}");
        let deps = deployments(a);
        assert!(deps.iter().all(|d| d.result == store::RESULT_FAILED && d.error_code.as_deref() == Some(code)), "{a}: {deps:?}");
        assert_eq!(with_rt(a, |rt| rt.backoff.recent_restarts()), 0, "{a}: K2's fault never feeds the crash backoff");
        let at = with_rt(a, |rt| rt.next_restart_at).expect("still scheduled");
        assert!(at - now() <= super::super::engine_gate::WAITING_RETRY_SECS, "{a}: retried every minute, never 30");
        let lines = lines_for(a);
        assert_eq!(lines.len(), 12, "{a}: an ERROR line per attempt: {lines:?}");
        assert!(lines.iter().all(|l| l.starts_with("ERROR [apps] ") && l.contains(&format!("failed ({code})")) && l.contains("next try on engine change")), "{lines:?}");
        let n = notices_for(a);
        assert!(n.iter().any(|t| t.contains("waiting for K2's App engine")), "{a}: the owner heard: {n:?}");
        assert_eq!(n.iter().filter(|t| t.contains("waiting for K2's App engine")).count(), 1, "once per run");
    }
    assert!(deployments(stopped).is_empty(), "an owner-stopped App never starts");

    // The heal installs the matching engine (any install path signals it).
    h1.set(OK);
    h3.set(OK);
    let t0 = Instant::now();
    super::super::engine_gate::engine_changed("test: k2-app-engine 0.46.4 installed");
    tick_until(&only, &seen, 10, "both live after the install", || live(a1) && live(a3));
    assert!(t0.elapsed() < Duration::from_secs(8), "back on the next tick, not the next backoff step");
    for a in [a1, a3] {
        let d = deployments(a);
        assert_eq!(d[0].result, store::RESULT_LIVE, "{a}");
        assert_eq!(d[0].kind, store::KIND_RESTART);
        assert!(app_row(a).state_reason.is_none());
    }
    assert!(deployments(stopped).is_empty(), "the kick never starts an owner-stopped App");
    assert_eq!(app_row(stopped).state, store::STATE_STOPPED);
}

/// T35 (R43): an App-caused crash restart waiting 30 min on its backoff,
/// and a K2-side failed App: a protocol that starts matching (the engine
/// `version` flips 2 → 3) kicks both at once, with the backoff reset.
#[test]
fn a_newly_matching_engine_kicks_pending_restarts_and_resets_backoff() {
    let f = fx();
    let eng = TestEngine::leak(OK);
    let (crashy, k2side) = ("s1t-kick-crash", "s1t-kick-failed");
    app(crashy, &f.seed, "restarting", eng);
    app(k2side, &f.seed, store::STATE_FAILED, eng);
    {
        let dep = db(|c| {
            store::insert_deployment(
                c,
                &store::NewDeployment { app_id: k2side, kind: store::KIND_START, from_seed: None, to_seed: Some(&f.seed), commit_ref: None, actor: "owner", reason: None },
            )
        })
        .unwrap();
        db(|c| store::set_failed(c, &dep, codes::ENGINE_PROTOCOL_MISMATCH, "protocol 2 vs 3")).unwrap();
    }
    with_rt(crashy, |rt| {
        for i in 0..5 {
            rt.backoff.on_crash(now() - 100 + i);
        }
        rt.next_restart_at = Some(now() + 1800);
    });
    let only = ids(&[crashy, k2side]);
    let seen = AtomicU64::new(super::super::engine_gate::generation());
    tick_with(&seen, Some(&only));
    assert!(deployments(crashy).is_empty(), "30 minutes away: nothing yet");
    // The engine watch's observations: protocol 2, then 3.
    super::super::engine_gate::observe(&Ok(serde_json::json!({ "version": "0.46.0", "protocol": 2 })));
    let t0 = now();
    let seen_kick = super::super::engine_gate::observe(&Ok(serde_json::json!({ "version": "0.46.4", "protocol": 3 })));
    assert_eq!(seen_kick, super::super::engine_gate::Seen::NewlyMatching);
    tick_until(&only, &seen, 10, "both restarts begun", || !deployments(crashy).is_empty() && !deployments(k2side).iter().all(|d| d.kind == store::KIND_START));
    for a in [crashy, k2side] {
        let d = deployments(a);
        assert!(d[0].started_at - t0 <= 1, "{a}: began within a second of the signal: {d:?}");
    }
    tick_until(&only, &seen, 10, "both live", || live(crashy) && live(k2side));
    assert_eq!(with_rt(crashy, |rt| rt.backoff.recent_restarts()), 0, "the backoff was reset");
}

/// T36 (R44): five failed restarts, App-caused first, then K2-side: five
/// ERROR lines each naming the next try; exactly one owner notice after the
/// third failed restart and one on entering `waiting_engine`; none again
/// until the App goes live and fails again.
#[test]
fn every_failed_restart_is_loud_and_the_owner_hears_once_per_run() {
    let f = fx();
    let eng = TestEngine::leak(APP_BROKEN);
    let id = "s1t-loud";
    app(id, &f.seed, store::STATE_RUNNING, eng);
    let only = ids(&[id]);
    boot_with(&answering(vec![]), false, Vec::new(), now(), Some(&only));
    let seen = AtomicU64::new(super::super::engine_gate::generation());
    for n in 1..=5usize {
        if n == 4 {
            eng.set(PROTOCOL_2);
        }
        if n > 1 {
            due_now(id);
        }
        tick_until(&only, &seen, 20, "a failed restart", || deployments(id).len() == n && idle(id));
        settle(id);
        let notices = notices_for(id);
        match n {
            1 | 2 => assert!(notices.is_empty(), "{n}: {notices:?}"),
            3 => assert_eq!(notices, vec![format!("App {id} failed to restart 3 times")]),
            _ => assert_eq!(notices, vec![format!("App {id} failed to restart 3 times"), format!("App {id} is waiting for K2's App engine")]),
        }
    }
    let lines = lines_for(id);
    assert_eq!(lines.len(), 5, "{lines:?}");
    let codes_seen: Vec<bool> = lines.iter().map(|l| l.contains(&format!("failed ({})", codes::HEALTH_FAILED))).collect();
    assert_eq!(codes_seen, vec![true, true, true, false, false], "{lines:?}");
    assert!(lines[0].ends_with("next try now") && lines[1].ends_with("next try in 5s") && lines[2].ends_with("next try in 30s"), "{lines:?}");
    assert!(lines[3].ends_with("next try on engine change (or in 60s)"), "{}", lines[3]);
    assert_eq!(with_rt(id, |rt| rt.backoff.recent_restarts()), 3, "only the App-caused failures count");
    // Live again: the run ends; a new run notices again.
    eng.set(OK);
    super::super::engine_gate::engine_changed("test install");
    tick_until(&only, &seen, 10, "live", || live(id));
    assert_eq!(notices_for(id).len(), 2);
}

/// R35: the desired-state boot rule.
#[test]
fn boot_restarts_what_the_owner_wants_running() {
    let f = fx();
    let eng = TestEngine::leak(OK);
    let boot = now();
    let fail = |id: &str, code: &str, finished: i64| {
        let dep = db(|c| {
            store::insert_deployment(
                c,
                &store::NewDeployment { app_id: id, kind: store::KIND_RESTART, from_seed: None, to_seed: Some(&f.seed), commit_ref: None, actor: "system", reason: None },
            )
        })
        .unwrap();
        db(|c| {
            store::set_failed(c, &dep, code, "x")?;
            c.execute("UPDATE deployments SET finished_at = ?2, started_at = ?2 WHERE id = ?1", params![dep, finished])
        })
        .unwrap();
    };
    for (id, state) in [
        ("s1t-r35-running", store::STATE_RUNNING),
        ("s1t-r35-restarting", "restarting"),
        ("s1t-r35-waiting", store::STATE_WAITING_ENGINE),
        ("s1t-r35-stopped", store::STATE_STOPPED),
        ("s1t-r35-parked", store::STATE_PARKED),
        ("s1t-r35-suspended", "suspended"),
    ] {
        app(id, &f.seed, state, eng);
    }
    app("s1t-r35-k2", &f.seed, store::STATE_CRASHLOOPING, eng);
    for i in 0..6 {
        fail("s1t-r35-k2", codes::ENGINE_PROTOCOL_MISMATCH, boot - 3000 + i * 60);
    }
    app("s1t-r35-appbad", &f.seed, store::STATE_CRASHLOOPING, eng);
    for i in 0..6 {
        fail("s1t-r35-appbad", codes::HEALTH_FAILED, boot - 3000 + i * 60);
    }
    app("s1t-r35-shutdown", &f.seed, store::STATE_FAILED, eng);
    fail("s1t-r35-shutdown", codes::HEALTH_FAILED, boot - 120);
    app("s1t-r35-mixed", &f.seed, store::STATE_FAILED, eng);
    fail("s1t-r35-mixed", codes::ENGINE_NOT_ANSWERING, boot - 2000);
    fail("s1t-r35-mixed", codes::HEALTH_FAILED, boot - 1900);
    app("s1t-r35-legacy", &f.seed, store::STATE_FAILED, eng);
    {
        let dep = db(|c| {
            store::insert_deployment(
                c,
                &store::NewDeployment { app_id: "s1t-r35-legacy", kind: store::KIND_RESTART, from_seed: None, to_seed: Some(&f.seed), commit_ref: None, actor: "system", reason: None },
            )
        })
        .unwrap();
        // A 0.46.3 row: no error_code, only the old text.
        db(|c| store::set_result(c, &dep, store::RESULT_FAILED, Some("engine_unavailable: the App engine speaks protocol 2 and this daemon 3: k2 app engine install")))
            .unwrap();
    }
    let want = |id: &str| wants_running(&app_row(id), boot);
    for id in ["s1t-r35-running", "s1t-r35-restarting", "s1t-r35-waiting", "s1t-r35-k2", "s1t-r35-shutdown", "s1t-r35-legacy"] {
        assert!(want(id), "{id} should start at boot");
    }
    for id in ["s1t-r35-stopped", "s1t-r35-parked", "s1t-r35-suspended", "s1t-r35-appbad", "s1t-r35-mixed"] {
        assert!(!want(id), "{id} stays down");
    }
}

// ── S2: re-host through the pipeline ─────────────────────────────────

/// Bring `id` live on the test engine (a first deploy) and return the
/// deployment id.
fn go_live_first(id: &str, seed: &str) -> String {
    let app = app_row(id);
    let dep = begin(&app, seed, store::KIND_DEPLOY, "owner", None).expect("deploy begins");
    let row = wait_done(&dep, 30).expect("row");
    assert_eq!(row.result, store::RESULT_LIVE, "{row:?}");
    dep
}

fn audit_events(app: &str) -> Vec<String> {
    let h = k2_core::db::shared();
    let c = h.lock();
    k2_core::deploy::audit::list_for(&c, k2_core::deploy::audit::SUBJECT_APP, app, 100).unwrap().into_iter().map(|r| r.event).collect()
}

/// R10 + A7 + R38: a re-host is the same Seed on a fresh instance; its
/// fingerprint is the `version` prepare checked; the old instance stays
/// warm; `k2 app rollback` with no `--to` goes back to that warm instance
/// by deployment id (never an older Seed), and with no warm instance left
/// it refuses `nothing_to_roll_back`.
#[test]
fn a_rehost_is_a_fresh_instance_with_a_fingerprint_and_rolls_back_to_warm() {
    let f = fx();
    let eng = TestEngine::leak(OK);
    let id = "s2t-rehost";
    app(id, &f.seed, store::STATE_INSTALLING, eng);
    let first = go_live_first(id, &f.seed);
    let fp1 = live_fingerprint(&app_row(id)).expect("a deploy writes a fingerprint too");
    assert_eq!((fp1.k2_version.as_str(), fp1.engine_version.as_deref()), (env!("CARGO_PKG_VERSION"), Some("9.9.9-test")));
    let dep = begin(&app_row(id), &f.seed, store::KIND_REHOST, "system", Some("K2 updated to 9.9.9")).unwrap();
    let row = wait_done(&dep, 30).unwrap();
    assert_eq!((row.result.as_str(), row.kind.as_str(), row.to_seed.as_deref()), (store::RESULT_LIVE, store::KIND_REHOST, Some(f.seed.as_str())));
    let a = app_row(id);
    assert_eq!(a.live_deployment.as_deref(), Some(dep.as_str()));
    assert!(live_fingerprint(&a).is_some());
    assert_eq!(with_rt(id, |rt| rt.warm.as_ref().map(|w| w.handle.deployment_id.clone())), Some(first.clone()), "the old instance is warm");
    // A7: back to the warm instance, by deployment id.
    let rb = rollback_to_same_seed_warm(&a, "owner").unwrap().expect("instant rollback");
    let a = app_row(id);
    assert_eq!(a.live_deployment.as_deref(), Some(first.as_str()), "the warm instance is live again");
    let rbrow = db(|c| store::get_deployment(c, &rb)).unwrap().unwrap();
    assert_eq!((rbrow.kind.as_str(), rbrow.to_seed.as_deref()), (store::KIND_ROLLBACK, Some(f.seed.as_str())));
    // Re-host again, let the watch window pass (no warm), then the route
    // refuses instead of reaching for an older Seed.
    let dep2 = begin(&a, &f.seed, store::KIND_REHOST, "system", None).unwrap();
    assert_eq!(wait_done(&dep2, 30).unwrap().result, store::RESULT_LIVE);
    let warm = with_rt(id, |rt| {
        rt.watch = None;
        rt.warm.take()
    });
    assert!(warm.is_some());
    k2_core::deploy::set_enabled(true);
    let r = super::super::routes::handle("/cli/app/rollback", super::super::routes::Who::Owner, format!(r#"{{"app":"{id}"}}"#).as_bytes(), &Default::default());
    assert!(r.status.starts_with("409"), "{} {}", r.status, r.body);
    assert!(r.body.contains("nothing_to_roll_back") && r.body.contains("re-hosted on K2"), "{}", r.body);
    assert_eq!(app_row(id).live_deployment.as_deref(), Some(dep2.as_str()), "nothing moved");
}

/// 0.46.8: nine Seeds went live; the sweep frees what is past the last 5 and
/// past its grace. A rollback to a freed one is a refusal that names each
/// Seed K2 keeps and a rollback would accept (never one next to be freed or
/// across a migration) and where the code came from; it moves nothing and
/// starts nothing. A Seed that never ran here, or only ever failed, stays
/// `seed_not_found`. History marks each row. A rollback with no `--to` goes
/// to what's kept, and refuses rather than reach further back when an older
/// K2 lost it.
#[test]
fn a_rollback_past_what_is_kept_names_what_is_left_and_history_marks_it() {
    let f = fx();
    let eng = TestEngine::leak(OK);
    let id = "s2t-freed";
    app(id, &f.seed, store::STATE_RUNNING, eng);
    k2_core::deploy::set_enabled(true);
    let hash = |i: u32| format!("{i:02x}{}", "0".repeat(62));
    let now = k2_core::deploy::now();
    {
        let h = k2_core::db::shared();
        let c = h.lock();
        let base = store::get_seed(&c, &f.seed).unwrap().unwrap();
        // The live Seed is at data revision 1; only s5 is older.
        c.execute("UPDATE seeds SET last_used_at = 1, data_revision = 1 WHERE hash = ?1", params![f.seed]).unwrap();
        let row = |seed: &str, at: i64, commit: &str, live: bool| {
            let d = store::insert_deployment(
                &c,
                &store::NewDeployment { app_id: id, kind: store::KIND_DEPLOY, from_seed: None, to_seed: Some(seed), commit_ref: Some(commit), actor: "owner", reason: None },
            )
            .unwrap();
            c.execute(
                "UPDATE deployments SET started_at = ?2, live_at = ?3, result = ?4 WHERE id = ?1",
                params![d, at, live.then_some(at), if live { "superseded" } else { "failed" }],
            )
            .unwrap();
        };
        for i in 0..8u32 {
            // s3 is past the last 5 but in its grace: stored, next to go.
            let last_used_at = if i == 3 { now } else { 1 };
            store::upsert_seed(&c, &store::SeedRow { hash: hash(i), last_used_at, data_revision: i64::from(i != 5), ..base.clone() }).unwrap();
            row(&hash(i), 86_400 * (i64::from(i) + 1), &format!("commit{i}abcdef0123"), true);
        }
        row(&f.seed, 86_400 * 9, &f.seed, true);
        // A failed deploy of a Seed still stored: not a target.
        store::upsert_seed(&c, &store::SeedRow { hash: hash(50), last_used_at: now, ..base.clone() }).unwrap();
        row(&hash(50), 86_400 * 10, "bad", false);
        // A deploy that only ever failed, its Seed gone: it never ran.
        row(&hash(60), 86_400 * 11, "bad2", false);
        // One Seed at a time: this DB is shared with other tests' Apps.
        for i in 0..8u32 {
            let d = store::detach_seed(&c, &hash(i), now, store::SWEEP_MAX_AGE_SECS, store::SWEEP_KEEP_DEPLOYMENTS).unwrap();
            assert_eq!(matches!(d, store::Detach::Moved(_)), i < 3, "s0..s2 go; s3 is in its grace; s4..s7 and the live one are the last 5: s{i}");
        }
    }
    let rows_before = deployments(id).len();
    let post = |body: serde_json::Value| super::super::routes::handle("/cli/app/rollback", super::super::routes::Who::Owner, body.to_string().as_bytes(), &Default::default());

    for to in [hash(1), hash(1)[..12].to_string()] {
        let r = post(serde_json::json!({ "app": id, "to": to }));
        assert!(r.status.starts_with("409"), "{} {}", r.status, r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["error"]["code"], codes::ROLLBACK_SEED_GONE, "{}", r.body);
        let hint = v["error"]["hint"].as_str().unwrap();
        for want in [
            "last 5 deployments",
            "3 days after they last ran",
            &format!("You can go back to {} (1970-01-09), {} (1970-01-08), {} (1970-01-06):", &hash(7)[..12], &hash(6)[..12], &hash(4)[..12]),
            &format!("k2 app rollback {id} --to <seed>"),
            &format!("k2 app history {id}"),
            "its source (commit commit1abcde)",
            "k2 app deploy",
        ] {
            assert!(hint.contains(want), "missing {want:?} in: {hint}");
        }
        assert!(!hint.contains(&hash(3)[..12]), "next to be freed: never named: {hint}");
        assert!(!hint.contains(&hash(5)[..12]), "across a migration: never named: {hint}");
    }
    for unknown in ["abcdef0123456789".to_string(), hash(60)] {
        let r = post(serde_json::json!({ "app": id, "to": unknown }));
        assert!(r.status.starts_with("404") && r.body.contains("seed_not_found"), "{unknown}: {} {}", r.status, r.body);
        assert!(r.body.contains("You can go back to") && !r.body.contains("last ran on"), "{}", r.body);
    }
    assert_eq!(deployments(id).len(), rows_before, "no refusal starts anything");
    assert_eq!(app_row(id).live_seed.as_deref(), Some(f.seed.as_str()), "nothing moved");

    let r = super::super::routes::handle(
        "/cli/app/history",
        super::super::routes::Who::Owner,
        b"",
        &std::collections::HashMap::from([("app".to_string(), id.to_string())]),
    );
    assert!(r.status.starts_with("200"), "{} {}", r.status, r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    let to: Vec<&str> = v["rollbackTo"].as_array().unwrap().iter().map(|t| t["seed"].as_str().unwrap()).collect();
    assert_eq!(to, vec![hash(7), hash(6), hash(4)], "{v}");
    assert_eq!((v["keepDeployments"].as_u64(), v["keepUnusedDays"].as_u64()), (Some(5), Some(3)), "{v}");
    let flag = |d: &serde_json::Value, k: &str| d[k].as_bool().unwrap_or_else(|| panic!("{k}: {d}"));
    let marks: Vec<(String, bool, bool, bool, bool)> = v["deployments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| (d["toSeed"].as_str().unwrap().to_string(), flag(d, "rollbackable"), flag(d, "seedStored"), flag(d, "freedSoon"), flag(d, "crossesMigration")))
        .collect();
    let want = vec![
        (hash(60), false, false, false, false),
        (hash(50), false, true, false, false),
        (f.seed.clone(), false, true, false, false),
        (hash(7), true, true, false, false),
        (hash(6), true, true, false, false),
        (hash(5), false, true, false, true),
        (hash(4), true, true, false, false),
        (hash(3), true, true, true, false),
        (hash(2), false, false, false, false),
        (hash(1), false, false, false, false),
        (hash(0), false, false, false, false),
    ];
    assert_eq!(marks, want, "newest first (seed, rollbackable, stored, freed soon, crosses)");

    // No --to: the newest earlier one, never the failed deploy.
    let a = app_row(id);
    assert_eq!(rollback_target(&a, None).unwrap(), hash(7));
    // A box an older K2 swept: refused, not a jump further back.
    db(|c| c.execute("DELETE FROM seeds WHERE hash = ?1", params![hash(7)])).unwrap();
    let e = rollback_target(&a, None).unwrap_err();
    assert_eq!(e.code, codes::ROLLBACK_SEED_GONE, "{}", e.message);
    assert!(e.message.contains(&format!("You can go back to {} ", &hash(6)[..12])), "{}", e.message);
}

/// A second Seed with its own tree, for a real deploy over `f.seed`.
fn second_seed(f: &Fx) -> String {
    let dir = f._home.path().join("src").join("app2");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("server.js"), "require('http').createServer((q,s)=>s.end('two')).listen(process.env.PORT)").unwrap();
    let h = k2_core::db::shared();
    let c = h.lock();
    let base = store::get_seed(&c, &f.seed).unwrap().unwrap();
    std::fs::write(dir.join("manifest.json"), base.manifest.to_string()).unwrap();
    let st = k2_core::deploy::seed::store_folder(&dir, &std::collections::BTreeSet::new()).unwrap();
    store::upsert_seed(&c, &store::SeedRow { hash: st.hash.clone(), ..base }).unwrap();
    st.hash
}

/// 0.46.8, through the real pipeline: a rollback someone chose through the
/// warm instance marks the outgoing deploy `rolled_back`, but it ran fine,
/// so it stays a rollback point, and its text survives the retire that
/// follows. A plain rollback goes back to it, as it always did.
#[test]
fn a_chosen_rollback_through_the_warm_instance_keeps_the_deploy_it_left() {
    let f = fx();
    let eng = TestEngine::leak(OK);
    let id = "s2t-chosen";
    app(id, &f.seed, store::STATE_INSTALLING, eng);
    let two = second_seed(&f);
    go_live_first(id, &f.seed);
    let second = begin(&app_row(id), &two, store::KIND_DEPLOY, "owner", None).unwrap();
    assert_eq!(wait_done(&second, 30).unwrap().result, store::RESULT_LIVE);
    rollback_to_warm(&app_row(id), &f.seed, "owner").unwrap().expect("the warm instance answers");
    let end = Instant::now() + Duration::from_secs(15);
    while !audit_events(id).iter().any(|e| e == "stopped") {
        assert!(Instant::now() < end, "the deploy it left must be retired: {:?}", audit_events(id));
        std::thread::sleep(Duration::from_millis(20));
    }
    let row = db(|c| store::get_deployment(c, &second)).unwrap().unwrap();
    assert_eq!((row.result.as_str(), row.error.as_deref()), (store::RESULT_ROLLED_BACK, Some(store::ROLLED_BACK_BY_CHOICE)));
    assert!(store::stayed_live(&row));
    let a = app_row(id);
    assert_eq!(a.live_seed.as_deref(), Some(f.seed.as_str()));
    assert_eq!(db(|c| store::went_live_seeds(c, id, 2)).unwrap(), vec![f.seed.clone(), two.clone()]);
    assert_eq!(rollback_target(&a, None).unwrap(), two);
    let targets = db(|c| store::rollback_targets(c, id, a.live_seed.as_deref(), store::SWEEP_KEEP_DEPLOYMENTS)).unwrap();
    assert!(targets.iter().any(|t| t.seed == two && t.usable()), "{targets:?}");
    settle(id);
}

/// 0.46.8: a Seed that goes between `begin`'s first read and its lock hold
/// (the sweep's detach getting in there) is refused by the check under the
/// lock: no deployment row, and the App isn't left busy. A start counts as a
/// use, so a Seed that then fails gets its full grace.
#[test]
fn begin_refuses_a_seed_freed_under_it_and_a_start_is_a_use() {
    let f = fx();
    let eng = TestEngine::leak(OK);
    let id = "s2t-begin-gone";
    app(id, &f.seed, store::STATE_RUNNING, eng);
    let before = deployments(id).len();
    let e = begin(&app_row(id), &"f".repeat(64), store::KIND_ROLLBACK, "owner", None).unwrap_err();
    assert_eq!(e.code, codes::SEED_NOT_FOUND, "{}", e.message);
    assert_eq!(deployments(id).len(), before, "no row for a Seed that isn't there");

    let going = second_seed(&f);
    let g2 = going.clone();
    let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let r2 = ran.clone();
    test_hooks::on_seed_read(id, move || {
        db(|c| c.execute("DELETE FROM seeds WHERE hash = ?1", params![g2])).unwrap();
        r2.store(true, Ordering::SeqCst);
    });
    let e = begin(&app_row(id), &going, store::KIND_DEPLOY, "owner", None).unwrap_err();
    assert!(ran.load(Ordering::SeqCst), "the first read saw the Seed, then it went");
    assert_eq!(e.code, codes::SEED_NOT_FOUND, "{}", e.message);
    assert_eq!(deployments(id).len(), before, "no row for the Seed that went");

    let t0 = k2_core::deploy::now();
    let dep = begin(&app_row(id), &f.seed, store::KIND_DEPLOY, "owner", None).expect("not held busy by the refusals");
    let used = db(|c| store::get_seed(c, &f.seed)).unwrap().unwrap().last_used_at;
    assert!(used >= t0, "the start touched the Seed: {used} < {t0}");
    wait_done(&dep, 30).expect("row");
    settle(id);
}

/// prd-app-pg-choice-dualrun-v1 PA10–PA11: a database switch's re-host is
/// a fresh instance through the gate with NOTHING warm (the old instance
/// held the closed source's DSN), records its epoch, is never an
/// auto-rollback target, and `k2 app rollback` with no `--to` answers
/// `db_switched` pointing at the database's own rollback.
#[test]
fn a_db_switch_rehost_keeps_nothing_warm_and_rollback_says_db_switched() {
    let f = fx();
    let eng = TestEngine::leak(OK);
    let id = "s2t-dbswitch";
    app(id, &f.seed, store::STATE_INSTALLING, eng);
    let first = go_live_first(id, &f.seed);
    // A deploy just before the move leaves a warm instance: retired too.
    let second = begin(&app_row(id), &f.seed, store::KIND_DEPLOY, "owner", None).unwrap();
    assert_eq!(wait_done(&second, 30).unwrap().result, store::RESULT_LIVE);
    assert_eq!(with_rt(id, |rt| rt.warm.as_ref().map(|w| w.handle.deployment_id.clone())), Some(first.clone()));
    let dep = begin_db_switch(&app_row(id), "owner", "mv_ws_jobs_20261024t100000z", false).unwrap();
    let row = wait_done(&dep, 30).unwrap();
    assert_eq!((row.result.as_str(), row.kind.as_str()), (store::RESULT_LIVE, store::KIND_REHOST));
    assert_eq!(row.reason.as_deref(), Some("db-switch mv_ws_jobs_20261024t100000z"));
    assert_eq!(db_switch_epoch(&row).as_deref(), Some("mv_ws_jobs_20261024t100000z"));
    assert!(with_rt(id, |rt| rt.warm.is_none()), "nothing warm after a database switch");
    assert!(!with_rt(id, |rt| rt.retire_on_live), "the flag is spent");
    settle(id);
    // Both stops run on background threads (retire; drain then stop).
    let end = Instant::now() + Duration::from_secs(15);
    while !(eng.stopped.lock().unwrap().contains(&first) && eng.stopped.lock().unwrap().contains(&second)) {
        assert!(Instant::now() < end, "the older warm instance and the replaced one must both stop: {:?}", eng.stopped.lock().unwrap());
        std::thread::sleep(Duration::from_millis(20));
    }
    let a = app_row(id);
    assert_eq!(live_db_switch(&a).as_deref(), Some("mv_ws_jobs_20261024t100000z"));
    assert_eq!(rollback_to_same_seed_warm(&a, "owner").unwrap(), None, "no warm rollback across a switch");
    k2_core::deploy::set_enabled(true);
    let r = super::super::routes::handle("/cli/app/rollback", super::super::routes::Who::Owner, format!(r#"{{"app":"{id}"}}"#).as_bytes(), &Default::default());
    assert!(r.status.starts_with("409"), "{} {}", r.status, r.body);
    assert!(r.body.contains(DB_SWITCHED) && r.body.contains("--rollback --yes"), "{}", r.body);
    assert_eq!(app_row(id).live_deployment.as_deref(), Some(dep.as_str()), "nothing moved");
    // An ordinary re-host is not a switch.
    let plain = begin(&app_row(id), &f.seed, store::KIND_REHOST, "system", Some("K2 updated to 9.9.9")).unwrap();
    let prow = wait_done(&plain, 30).unwrap();
    assert_eq!(db_switch_epoch(&prow), None);
    assert_eq!(live_db_switch(&app_row(id)), None);
}

/// PA12: the kind of the pipeline in flight, for the move's re-host check.
#[test]
fn in_flight_kind_names_a_running_rehost() {
    let f = fx();
    let eng = TestEngine::leak(OK);
    let id = "s2t-inflight";
    app(id, &f.seed, store::STATE_INSTALLING, eng);
    go_live_first(id, &f.seed);
    assert_eq!(in_flight_kind(id), None);
    let dep = begin(&app_row(id), &f.seed, store::KIND_REHOST, "system", None).unwrap();
    assert_eq!(wait_done(&dep, 30).unwrap().result, store::RESULT_LIVE);
    settle(id);
    assert_eq!(in_flight_kind(id), None, "done: nothing in flight");
    // The pipeline's own state while that re-host runs (busy + in flight).
    with_rt(id, |rt| {
        rt.busy = true;
        rt.in_flight = Some(dep.clone());
    });
    assert_eq!(in_flight_kind(id).as_deref(), Some(store::KIND_REHOST));
    with_rt(id, |rt| {
        rt.busy = false;
        rt.in_flight = None;
    });
    assert_eq!(in_flight_kind(id), None);
}

/// A13: a failed re-host wakes nobody (no deploy_failed wake), writes its
/// R44 line, and the old instance keeps serving.
#[test]
fn a_failed_rehost_is_quiet_and_keeps_the_old_instance() {
    let f = fx();
    let woke: std::sync::Arc<Mutex<Vec<String>>> = Default::default();
    let w2 = woke.clone();
    super::super::alerts::set_sink(Some(Box::new(move |ws: &str, text: &str| w2.lock().unwrap().push(format!("{ws}: {text}")))));
    let eng = TestEngine::leak(OK);
    let id = "s2t-quiet";
    app(id, &f.seed, store::STATE_INSTALLING, eng);
    let first = go_live_first(id, &f.seed);
    eng.set(APP_BROKEN);
    let dep = begin(&app_row(id), &f.seed, store::KIND_REHOST, "system", None).unwrap();
    let row = wait_done(&dep, 30).unwrap();
    assert_eq!((row.result.as_str(), row.error_code.as_deref()), (store::RESULT_FAILED, Some(codes::HEALTH_FAILED)));
    assert_eq!(app_row(id).live_deployment.as_deref(), Some(first.as_str()), "the old instance still serves");
    assert!(live(id));
    assert!(woke.lock().unwrap().iter().all(|m| !m.contains(id)), "no wake for a re-host: {:?}", woke.lock().unwrap());
    assert!(lines_for(id).iter().any(|l| l.contains(&format!("{id} rehost {dep} failed"))), "{:?}", lines_for(id));
}

/// Rosson Q4 + Q16 "pause briefly and update": the old instance stops
/// first, then the new one starts; when the new one fails, the live Seed
/// starts again by itself (the App never stays down).
#[test]
fn a_paused_rehost_stops_first_and_comes_back_if_the_new_instance_fails() {
    let f = fx();
    let eng = TestEngine::leak(OK);
    let id = "s2t-pause";
    app(id, &f.seed, store::STATE_INSTALLING, eng);
    let first = go_live_first(id, &f.seed);
    set_stop_first(id);
    let dep = begin(&app_row(id), &f.seed, store::KIND_REHOST, "system", None).unwrap();
    assert_eq!(wait_done(&dep, 30).unwrap().result, store::RESULT_LIVE);
    assert!(eng.stopped.lock().unwrap().contains(&first), "the old instance stopped first");
    assert!(with_rt(id, |rt| rt.warm.is_none()), "nothing warm after a pause");
    // Again, and the new instance is broken: the live Seed comes back.
    eng.set(APP_BROKEN);
    set_stop_first(id);
    let dep2 = begin(&app_row(id), &f.seed, store::KIND_REHOST, "system", None).unwrap();
    assert_eq!(wait_done(&dep2, 30).unwrap().result, store::RESULT_FAILED);
    settle(id);
    assert!(eng.stopped.lock().unwrap().contains(&dep), "it stopped the live one first");
    assert!(with_rt(id, |rt| rt.next_restart_at.is_some()), "a restart of the live Seed is scheduled");
    assert!(audit_events(id).iter().any(|e| e == "rehost_restore"));
    eng.set(OK);
    let only = ids(&[id]);
    let seen = AtomicU64::new(super::super::engine_gate::generation());
    tick_until(&only, &seen, 15, "the live Seed is back", || live(id));
}

// ── 0.46.7: static Apps (prd-tauri-core-0467-v1 TC-2, TC-4; TA-4, TA-5) ──

/// Store a static Seed (`host.mode: "static"`, `root: dist`) whose page
/// says `body`; its hash.
fn static_seed(home: &std::path::Path, tag: &str, body: &str) -> String {
    let f = home.join("src").join(tag);
    std::fs::create_dir_all(f.join("dist")).unwrap();
    std::fs::write(f.join("dist/index.html"), body).unwrap();
    let manifest = serde_json::json!({
        "schema": 1, "kind": "app", "id": "st", "name": "st", "version": "1.0.0",
        "host": { "mode": "static", "root": "dist" }
    });
    std::fs::write(f.join("manifest.json"), manifest.to_string()).unwrap();
    let st = k2_core::deploy::seed::store_folder(&f, &std::collections::BTreeSet::new()).unwrap();
    let h = k2_core::db::shared();
    let c = h.lock();
    store::upsert_seed(
        &c,
        &store::SeedRow {
            hash: st.hash.clone(),
            seed_id: "st".into(),
            version: Some("1.0.0".into()),
            host_mode: "static".into(),
            manifest,
            created_with: serde_json::json!({}),
            report: serde_json::json!({}),
            signer: "owner".into(),
            origin: "build".into(),
            built_for: None,
            built_from: None,
            source_ref: None,
            data_revision: 0,
            size_bytes: st.size_bytes as i64,
            created_at: 1,
            last_used_at: 1,
        },
    )
    .unwrap();
    st.hash
}

/// `GET /` on a loopback port: (status line, body).
fn http_get(port: u16) -> (String, String) {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(b"GET / HTTP/1.1\r\nHost: st.box.k2.dev\r\nConnection: close\r\n\r\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap_or((&out, ""));
    (head.lines().next().unwrap_or("").to_string(), body.to_string())
}

/// TC-2 + TA-5: a static Seed deploys on the static engine whatever the
/// App's isolation says (no engine install, no `engine_unavailable`, never
/// recorded as a weaker wall), goes live, keeps the previous Seed warm for
/// an instant rollback, and stops.
#[test]
fn a_static_seed_deploys_on_the_static_engine_and_keeps_the_previous_warm() {
    let f = fx();
    let eng = TestEngine::leak(OK);
    let id = "s7t-static-deploy";
    let one = static_seed(f._home.path(), "one", "<p>one</p>");
    let two = static_seed(f._home.path(), "two", "<p>two</p>");
    app(id, &one, store::STATE_INSTALLING, eng);
    let first = go_live_first(id, &one);
    assert_eq!(eng.starts.load(Ordering::SeqCst), 0, "the App's own engine never runs a static Seed");
    let port = router::live(id).expect("routed").port;
    assert_eq!(http_get(port), ("HTTP/1.1 200 OK".to_string(), "<p>one</p>".to_string()));
    assert_eq!(runtime_json(id)["engine"], "static");
    assert!(is_static_app(&app_row(id)));
    assert_eq!(isolation_shown(&app_row(id)), super::super::static_engine::ISOLATION_NOTE);
    assert!(!runs_unwalled(&app_row(id)), "a static App never counts as unwalled");
    assert_eq!(live_fingerprint(&app_row(id)).expect("fingerprint").wall, "static");
    // A second Seed: the first stays warm (and answering) through the
    // watch window.
    let second = go_live_first(id, &two);
    let warm = with_rt(id, |rt| rt.warm.as_ref().map(|w| (w.handle.deployment_id.clone(), w.handle.port))).expect("warm");
    assert_eq!(warm.0, first);
    assert_eq!(http_get(warm.1).1, "<p>one</p>", "the warm Seed still answers: rollback is instant");
    assert_eq!(http_get(router::live(id).unwrap().port).1, "<p>two</p>");
    assert_ne!(first, second);
    stop_app(&app_row(id), "owner").unwrap();
    assert!(router::live(id).is_none());
    assert_eq!(super::super::static_engine::engine().liveness(&RunHandle { app_id: id.into(), deployment_id: first, port: warm.1, pid: None, engine_ref: "static:".into() }), Liveness::Exited(None), "the warm instance stopped too");
    forget(id);
}

/// TC-4 + TA-4: after a daemon restart a live static App is served again
/// in line, before any engine wait, on its live deployment (no new
/// deployment, no restart scheduled); its front then answers 200, never
/// 503. A stopped static App stays down.
#[test]
fn static_apps_are_served_at_boot_before_the_engine_and_never_503() {
    let f = fx();
    let eng = TestEngine::leak(OK);
    let id = "s7t-static-boot";
    let down = "s7t-static-down";
    let seed = static_seed(f._home.path(), "boot", "<p>back</p>");
    app(id, &seed, store::STATE_RUNNING, eng);
    app(down, &seed, store::STATE_STOPPED, eng);
    // Its live deployment from before the restart.
    let live_dep = db(|c| {
        let d = store::insert_deployment(
            c,
            &store::NewDeployment { app_id: id, kind: store::KIND_DEPLOY, from_seed: None, to_seed: Some(&seed), commit_ref: None, actor: "owner", reason: None },
        )?;
        store::set_result(c, &d, store::RESULT_LIVE, None)?;
        store::set_live(c, id, Some(&seed), Some(&d))?;
        Ok(d)
    })
    .unwrap();
    let only = ids(&[id, down]);
    let served = serve_static_at_boot(now(), Some(&only));
    assert_eq!(served, vec![id.to_string()], "only the App the owner wants running");
    assert!(router::live(down).is_none());
    let be = router::live(id).expect("routed before any front opens");
    assert_eq!(http_get(be.port), ("HTTP/1.1 200 OK".to_string(), "<p>back</p>".to_string()));
    assert_eq!(with_rt(id, |rt| rt.live.as_ref().map(|i| i.handle.deployment_id.clone())), Some(live_dep.clone()));
    assert_eq!(deployments(id).len(), 1, "served again on its live deployment, no new one");
    assert!(audit_events(id).iter().any(|e| e == "static_served"));
    assert_eq!(live_fingerprint(&app_row(id)).expect("fingerprint").k2_version, env!("CARGO_PKG_VERSION"), "the re-host wave sees it current");
    // The rest of boot (an engine that answers at once) leaves it be.
    boot_with(&answering(vec![]), false, vec![], now(), Some(&only));
    assert!(with_rt(id, |rt| rt.next_restart_at.is_none()), "no restart for a served static App");
    let front = ensure_front(&app_row(id)).expect("front");
    assert_eq!(http_get(front).0, "HTTP/1.1 200 OK", "through the front: 200, never 503");
    stop_app(&app_row(id), "owner").unwrap();
    forget(id);
    forget(down);
}

// ── a held deploy, approved: the manifest's box services ─────────────

/// Field report: a deploy whose manifest added `boxServices` was held (new
/// caps) and approved; the App went live without them, and only a second
/// restart after a separate request gave it `K2_BOX_*`. The deploy path files
/// a manifest's services before it starts, and the path that carries out an
/// approved hold didn't. Now the hold's card names them, and approving it
/// covers them: the one start has the forwards, the env and the door. The
/// hosts the same manifest asks for are filed too, as on any deploy.
#[test]
fn an_approved_held_deploy_starts_with_its_manifests_box_services() {
    use k2_core::app_grace::requests as reqs;
    use k2_core::deploy::box_service as bs;
    let f = fx();
    k2_core::deploy::set_enabled(true);
    let id = "s1t-held-box";
    let eng = TestEngine::leak(OK);
    app(id, &f.seed, "running", eng);
    {
        let h = k2_core::db::shared();
        let c = h.lock();
        crate::apps::grants::record(&c, id, &[]).unwrap();
    }

    // The Seed: a new cap (so the deploy is held), two box services, a host.
    let src = f._home.path().join("src").join("held-box");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("server.js"), "require('http').createServer((q,s)=>s.end('ok')).listen(process.env.PORT)").unwrap();
    let manifest = serde_json::json!({
        "schema": 1, "kind": "app", "id": "s1", "name": "s1", "version": "2.0.0",
        "caps": ["thread:read"],
        "boxServices": [{ "name": "sew-api", "hostPort": 18470 }, { "name": "sew-signal", "hostPort": 17880 }],
        "egress": { "hosts": ["api.example.com"] },
        "host": { "mode": "cmd", "start": ["node", "server.js"], "health": { "path": "/", "timeoutSecs": 2 } }
    });
    std::fs::write(src.join("manifest.json"), manifest.to_string()).unwrap();
    let stored = k2_core::deploy::seed::store_folder(&src, &std::collections::BTreeSet::new()).unwrap();
    {
        let h = k2_core::db::shared();
        let c = h.lock();
        store::upsert_seed(
            &c,
            &store::SeedRow {
                hash: stored.hash.clone(),
                seed_id: "s1".into(),
                version: Some("2.0.0".into()),
                host_mode: "cmd".into(),
                manifest,
                created_with: serde_json::json!({}),
                report: serde_json::json!({ "ok": true }),
                signer: "owner".into(),
                origin: "build".into(),
                built_for: None,
                built_from: None,
                source_ref: None,
                data_revision: 0,
                size_bytes: stored.size_bytes as i64,
                created_at: 1,
                last_used_at: 1,
            },
        )
        .unwrap();
    }

    // Held: nothing filed, nothing started, and the card names them.
    let r = crate::apps::routes::handle(
        "/cli/app/deploy",
        crate::apps::routes::Who::Owner,
        serde_json::json!({ "app": id, "seed": stored.hash }).to_string().as_bytes(),
        &std::collections::HashMap::new(),
    );
    let held: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(held["status"], "held", "{held}");
    assert_eq!(held["kind"], "deploy", "{held}");
    assert!(crate::apps::box_service_routes::allowed_ports(id).is_empty(), "nothing opens while held");
    assert_eq!(eng.starts.load(Ordering::SeqCst), 0, "the live Seed keeps serving");

    // The owner approves the one card.
    let req_id = held["request"]["id"].as_str().expect("request id").to_string();
    let req = {
        let h = k2_core::db::shared();
        let c = h.lock();
        reqs::get(&c, &req_id).unwrap().expect("the held request")
    };
    let out = crate::apps::approvals::apply_as(&req, "owner", "local-auth").expect("approved");
    assert_eq!(out["request"]["status"], "approved", "{out}");
    let dep = out["result"]["deployment"].as_str().unwrap_or_else(|| panic!("no deployment: {out}"));
    let row = wait_done(dep, 30).expect("row");
    assert_eq!(row.result, store::RESULT_LIVE, "{row:?}");

    // The one start had everything: no second restart.
    assert_eq!(eng.starts.load(Ordering::SeqCst), 1, "one start");
    let seen = eng.box_at_start.lock().unwrap()[0].clone();
    assert_eq!(seen.forwards, vec![(18470, 18470), (17880, 17880)], "{seen:?}");
    assert_eq!(
        seen.env,
        vec![("K2_BOX_SEW_API".to_string(), "127.0.0.1:18470".to_string()), ("K2_BOX_SEW_SIGNAL".to_string(), "127.0.0.1:17880".to_string())],
        "{seen:?}"
    );
    assert_eq!(seen.door, vec![18470, 17880], "the firewall rule opens both at start: {seen:?}");

    // The owner saw what they approved, and the approval says what it did.
    let summary = held["request"]["summary"].as_str().expect("summary");
    assert!(summary.contains("sew-api") && summary.contains("sew-signal"), "the owner sees what they approve: {summary}");
    let lines = held["diff"]["lines"].to_string();
    assert!(lines.contains("sew-api") && lines.contains("sew-signal"), "{held}");
    assert_eq!(out["result"]["boxServices"]["allowed"], serde_json::json!(["sew-api", "sew-signal"]), "{out}");

    // On the list as allowed, approved by the owner; no second card for them,
    // and the hosts wait for the owner as on any deploy.
    let h = k2_core::db::shared();
    let c = h.lock();
    let rows = bs::list(&c, id).unwrap();
    assert_eq!(rows.iter().map(|r| (r.name.as_str(), r.status.as_str())).collect::<Vec<_>>(), vec![("sew-api", "allowed"), ("sew-signal", "allowed")]);
    let open = reqs::list(&c, &reqs::Filter { app_id: Some(id), status: Some(reqs::STATUS_PENDING), ..Default::default() }, 100).unwrap();
    assert_eq!(open.iter().map(|r| r.kind.as_str()).collect::<Vec<_>>(), vec![reqs::KIND_EGRESS], "{open:?}");
}
