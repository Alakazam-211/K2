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
}

impl TestEngine {
    fn leak(mode: u8) -> &'static TestEngine {
        Box::leak(Box::new(TestEngine { mode: AtomicU8::new(mode), prepares: AtomicU32::new(0), starts: AtomicU32::new(0), stopped: Mutex::new(HashSet::new()) }))
    }
    fn set(&self, mode: u8) {
        self.mode.store(mode, Ordering::SeqCst);
    }
}

/// A loopback HTTP server answering 200 to everything (the App).
fn serve(port: u16) -> std::io::Result<()> {
    let l = std::net::TcpListener::bind(("127.0.0.1", port))?;
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
        if self.mode.load(Ordering::SeqCst) != APP_BROKEN {
            serve(port).map_err(|e| EngineError::new(codes::ENGINE_UNAVAILABLE, e.to_string()))?;
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
