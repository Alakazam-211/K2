//! The re-host wave's rules over a fake box (update continuity S2: T5,
//! T6, T7, T22, T32). The pipeline itself (`rehost` kind, fingerprint,
//! pause, quiet failures, rollback to the warm instance) is exercised with
//! a test engine in `supervisor_tests.rs`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use super::*;

fn row(id: &str) -> AppRow {
    AppRow {
        app_id: id.into(),
        service_id: format!("svc-{id}"),
        seed_id: Some(id.into()),
        owner_workspace: format!("ws-{id}"),
        path: "guarded".into(),
        source: "seed".into(),
        folder_path: None,
        live_seed: Some(format!("seed-{id}")),
        live_tree_hash: None,
        live_deployment: Some(format!("d-{id}")),
        preview_seed: None,
        isolation: "container".into(),
        memory_limit_bytes: None,
        front_port: Some(41000),
        state: "running".into(),
        state_reason: None,
        created_by: "owner".into(),
        created_at: 0,
        updated_at: 0,
        io_max_json: None,
        devices_json: "[]".into(),
        exception_json: None,
        rehost_policy: "auto".into(),
        rehost_window: None,
        rehost_when_no_overlap: "pause".into(),
        warm_deployment: None,
        warm_until: None,
    }
}

fn fp(k2: &str) -> rh::Fingerprint {
    rh::Fingerprint { k2_version: k2.into(), engine_version: Some(k2.into()), engine_protocol: Some(3), ..Default::default() }
}

#[derive(Default)]
struct Fake {
    apps: Vec<AppRow>,
    fps: HashMap<String, rh::Fingerprint>,
    conns: HashMap<String, usize>,
    busy: Vec<String>,
    requires_bad: Vec<String>,
    memory_sessions: Vec<String>,
    shape: HashMap<String, String>,
    no_room: Vec<String>,
    held: Vec<String>,
    fail: Vec<String>,
    off: bool,
    minute: u32,
    draining: Cell<u32>,
    core: RefCell<Vec<Option<String>>>,
    /// `core_pending` answers, one per call, then a fixed answer.
    pending: RefCell<Vec<Option<String>>>,
    pending_after: Option<String>,
    slept: RefCell<Vec<Duration>>,
    rehosted: RefCell<Vec<(String, bool)>>,
    audits: RefCell<Vec<(String, String, J)>>,
    owner: RefCell<Vec<String>>,
    agent: RefCell<Vec<(String, String)>>,
}

impl WaveOps for Fake {
    fn apps(&self) -> Vec<AppRow> {
        self.apps.clone()
    }
    fn k2_version(&self) -> String {
        "0.46.4".into()
    }
    fn engine_version(&self) -> Option<String> {
        Some("0.46.4".into())
    }
    fn engine_ready(&self) -> bool {
        true
    }
    fn predecessor_draining(&self) -> bool {
        let n = self.draining.get();
        if n > 0 {
            self.draining.set(n - 1);
        }
        n > 0
    }
    fn core_busy(&self) -> Option<String> {
        let mut c = self.core.borrow_mut();
        if c.is_empty() {
            None
        } else {
            c.remove(0)
        }
    }
    fn core_pending(&self) -> Option<String> {
        let mut c = self.pending.borrow_mut();
        if c.is_empty() {
            self.pending_after.clone()
        } else {
            c.remove(0)
        }
    }
    fn minute_of_day(&self) -> u32 {
        self.minute
    }
    fn rehost_off(&self) -> bool {
        self.off
    }
    fn sleep(&self, d: Duration) {
        self.slept.borrow_mut().push(d);
    }
    fn live_fingerprint(&self, app: &AppRow) -> Option<rh::Fingerprint> {
        self.fps.get(&app.app_id).cloned()
    }
    fn runs_unwalled(&self, app: &AppRow) -> bool {
        app.isolation == "process" && app.path != "open"
    }
    fn busy(&self, app: &AppRow) -> bool {
        self.busy.contains(&app.app_id)
    }
    fn active_connections(&self, app: &AppRow) -> usize {
        self.conns.get(&app.app_id).copied().unwrap_or(0)
    }
    fn requires(&self, app: &AppRow) -> Result<(), String> {
        if self.requires_bad.contains(&app.app_id) {
            Err("this App needs Sew >=0.4.0 and the box has 0.3.0".into())
        } else {
            Ok(())
        }
    }
    fn sessions(&self, app: &AppRow) -> SessionStore {
        if self.memory_sessions.contains(&app.app_id) {
            SessionStore::Memory
        } else {
            SessionStore::Sealed
        }
    }
    fn no_overlap_shape(&self, app: &AppRow) -> Option<String> {
        self.shape.get(&app.app_id).cloned()
    }
    fn headroom(&self, app: &AppRow) -> Result<(), String> {
        if self.no_room.contains(&app.app_id) {
            Err("the live instance uses 900 MiB and 300 MiB is free after the 512 MiB floor".into())
        } else {
            Ok(())
        }
    }
    fn held_failed(&self, app: &AppRow) -> bool {
        self.held.contains(&app.app_id)
    }
    fn rehost(&self, app: &AppRow, pause: bool, _reason: &str) -> Outcome {
        self.rehosted.borrow_mut().push((app.app_id.clone(), pause));
        if self.fail.contains(&app.app_id) {
            Outcome::Failed(format!("d-new-{}", app.app_id), "health_failed: GET /healthz never answered".into())
        } else {
            Outcome::Live(format!("d-new-{}", app.app_id))
        }
    }
    fn audit(&self, app: &str, event: &str, payload: J) {
        self.audits.borrow_mut().push((app.into(), event.into(), payload));
    }
    fn owner_notice(&self, title: &str, _body: &str) {
        self.owner.borrow_mut().push(title.into());
    }
    fn agent_note(&self, app: &AppRow, text: &str) {
        self.agent.borrow_mut().push((app.app_id.clone(), text.into()));
    }
}

/// The wave's status and breaker are process-wide: one wave test at a time.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static M: Mutex<()> = Mutex::new(());
    M.lock().unwrap_or_else(|e| e.into_inner())
}

fn events(f: &Fake, app: &str) -> Vec<String> {
    f.audits.borrow().iter().filter(|(a, _, _)| a == app).map(|(_, e, _)| e.clone()).collect()
}

/// T5: due by fingerprint (another K2, none at all), nothing for a current
/// or unwalled or stopped App; fewest connections first; 10 s between Apps.
#[test]
fn the_wave_moves_due_apps_fewest_connections_first() {
    let _g = serial();
    let mut f = Fake::default();
    let (a, b, c, cur, unw, stopped) = ("w5-a", "w5-b", "w5-c", "w5-cur", "w5-unw", "w5-stopped");
    f.apps = vec![row(a), row(b), row(c), row(cur), AppRow { isolation: "process".into(), ..row(unw) }, AppRow { state: "stopped".into(), ..row(stopped) }];
    f.fps.insert(a.into(), fp("0.46.3"));
    f.fps.insert(b.into(), fp("0.46.3"));
    // c: no fingerprint (a row from before 0221) = due.
    f.fps.insert(cur.into(), fp("0.46.4"));
    f.conns.insert(a.into(), 7);
    f.conns.insert(b.into(), 0);
    f.conns.insert(c.into(), 3);
    let turns = run_wave(&f);
    assert_eq!(turns.iter().map(|(a, _)| a.as_str()).collect::<Vec<_>>(), vec![b, c, a], "fewest active connections first");
    assert!(turns.iter().all(|(_, t)| matches!(t, Turn::Done(_))));
    assert_eq!(*f.rehosted.borrow(), vec![(b.into(), false), (c.into(), false), (a.into(), false)], "no pause when two copies fit");
    assert_eq!(f.slept.borrow().iter().filter(|d| **d == GAP).count(), 2, "10 s between Apps");
    assert!(events(&f, cur).is_empty() && events(&f, unw).is_empty() && events(&f, stopped).is_empty());
    assert_eq!(events(&f, b), vec!["rehost_due", "rehost_done"]);
    assert_eq!(status_json()["state"], "done");
}

/// T22: pre-flights. The predecessor drains → the wave waits; core
/// upgrades → it waits (bounded); `requires` unmet, sessions in memory and
/// a busy App are deferred with their code and the old instance untouched;
/// `--yes` forces the sessions case.
#[test]
fn pre_flights_wait_and_defer() {
    let _g = serial();
    let mut f = Fake::default();
    let (req, mem, busy, ok) = ("w22-req", "w22-mem", "w22-busy", "w22-ok");
    f.apps = vec![row(req), row(mem), row(busy), row(ok)];
    f.requires_bad = vec![req.into()];
    f.memory_sessions = vec![mem.into()];
    f.busy = vec![busy.into()];
    f.draining.set(3);
    *f.core.borrow_mut() = vec![Some("a Sew upgrade".into()), Some("a Sew upgrade".into())];
    let turns: HashMap<String, Turn> = run_wave(&f).into_iter().collect();
    assert_eq!(f.slept.borrow().iter().filter(|d| **d == Duration::from_secs(5)).count(), 3, "waited while the predecessor drained");
    // Two 10 s waits for the core upgrade, then three 10 s gaps between the four Apps.
    assert_eq!(f.slept.borrow().iter().filter(|d| **d == Duration::from_secs(10)).count(), 2 + 3, "waited for the core upgrade");
    assert!(matches!(turns[req], Turn::Deferred(defer::REQUIRES_UNMET, _)));
    assert!(matches!(turns[mem], Turn::Deferred(defer::SESSIONS_IN_MEMORY, _)));
    assert!(matches!(turns[busy], Turn::Deferred(defer::BUSY, _)));
    assert!(matches!(turns[ok], Turn::Done(_)));
    assert_eq!(*f.rehosted.borrow(), vec![(ok.into(), false)], "deferred Apps are never touched");
    assert_eq!(f.owner.borrow().iter().filter(|t| t.contains(mem)).count(), 1, "the sessions card (Q9: defer and ask)");
    // The owner's `k2 app rehost <app> --yes`.
    assert!(matches!(turn(&f, &row(mem), Some(Forced { yes: true, pause: false })), Turn::Done(_)));
    // The core wait is bounded at 30 minutes.
    let g = Fake { apps: vec![row("w22-late")], ..Default::default() };
    *g.core.borrow_mut() = vec![Some("a Postgres move".into()); 400];
    let t = run_wave(&g);
    assert_eq!(g.slept.borrow().iter().map(|d| d.as_secs()).sum::<u64>(), CORE_WAIT.as_secs());
    assert!(matches!(t[0].1, Turn::Done(_)), "it starts after 30 minutes anyway");
}

/// R16: `k2 app list`'s engine column words.
#[test]
fn the_engine_column_names_what_the_app_went_live_on() {
    assert_eq!(engine_label(false, None), "-", "nothing live");
    assert_eq!(engine_label(true, None), "unknown", "a row from before 0221");
    assert_eq!(engine_label(true, Some(&fp("0.46.3"))), "0.46.3");
    let unwalled = rh::Fingerprint { k2_version: "0.46.4".into(), wall: "unwalled".into(), ..Default::default() };
    assert_eq!(engine_label(true, Some(&unwalled)), "unwalled, K2 0.46.4");
    let bare = rh::Fingerprint { k2_version: "0.46.4".into(), ..Default::default() };
    assert_eq!(engine_label(true, Some(&bare)), "no engine, K2 0.46.4");
}

/// R29 (Sew C): the wave also waits for a newer pinned Sew that waits to
/// install, not only for an upgrade that already runs; bounded at 30
/// minutes, and a pending pin that was waited out once isn't waited for
/// again by the next (hourly) wave.
#[test]
fn the_wave_waits_for_a_pending_sew_pin() {
    let _g = serial();
    // The pin installs after two checks: two 10 s waits, then the wave.
    let f = Fake { apps: vec![row("w29-a")], ..Default::default() };
    let pin = "a pending Sew upgrade (0.3.0 to the pinned 0.3.1) [w29-a]";
    *f.pending.borrow_mut() = vec![Some(pin.into()), Some(pin.into())];
    let t = run_wave(&f);
    assert_eq!(*f.slept.borrow(), vec![Duration::from_secs(10), Duration::from_secs(10)], "waited for the pending pin");
    assert!(matches!(t[0].1, Turn::Done(_)));
    assert!(!f.audits.borrow().iter().any(|(_, e, _)| e == "rehost_core_wait"), "it installed in time");

    // A running upgrade, then the pending pin of the same core: both waited.
    let f = Fake { apps: vec![row("w29-b")], ..Default::default() };
    *f.core.borrow_mut() = vec![Some("a Sew upgrade".into())];
    *f.pending.borrow_mut() = vec![Some(format!("{pin} b"))];
    run_wave(&f);
    assert_eq!(f.slept.borrow().len(), 2, "one wait for the running upgrade, one for the pending pin");

    // A pin nobody installs: 30 minutes, the wave starts and says why.
    let never = "a pending Sew upgrade (0.3.0 to the pinned 0.3.1) [w29-never]";
    let f = Fake { apps: vec![row("w29-c")], pending_after: Some(never.into()), ..Default::default() };
    let t = run_wave(&f);
    assert_eq!(f.slept.borrow().iter().map(|d| d.as_secs()).sum::<u64>(), CORE_WAIT.as_secs());
    assert!(matches!(t[0].1, Turn::Done(_)), "it starts after 30 minutes anyway");
    let audits = f.audits.borrow();
    let (_, _, why) = audits.iter().find(|(_, e, _)| e == "rehost_core_wait").expect("the wait is audited");
    assert_eq!(why["what"], never);
    assert_eq!(why["pending"], true);
    drop(audits);
    // The next wave doesn't wait for the same pending pin again …
    let f = Fake { apps: vec![row("w29-d")], pending_after: Some(never.into()), ..Default::default() };
    run_wave(&f);
    assert!(f.slept.borrow().is_empty(), "a pin waited out once isn't waited for again: {:?}", f.slept.borrow());
    // … but a newer one is.
    let f = Fake { apps: vec![row("w29-e")], ..Default::default() };
    *f.pending.borrow_mut() = vec![Some(format!("{never} newer"))];
    run_wave(&f);
    assert_eq!(*f.slept.borrow(), vec![Duration::from_secs(10)]);
}

/// The live probe: a pending pin is a released pin newer than the installed
/// Sew on a box where Sew is installed; nothing otherwise.
#[test]
fn a_pending_sew_pin_is_a_newer_pin_than_the_installed_sew() {
    use crate::sew::store::Row;
    use crate::sew::supervisor::pin_pending_for;
    let row = |status: &str, installed: Option<&str>| Row { status: status.into(), installed_version: installed.map(Into::into), ..Default::default() };
    assert_eq!(
        pin_pending_for(Some(&row("running", Some("0.3.0"))), "0.3.1").as_deref(),
        Some("a pending Sew upgrade (0.3.0 to the pinned 0.3.1)")
    );
    for status in ["degraded", "stopped", "upgrading"] {
        assert!(pin_pending_for(Some(&row(status, Some("0.3.0"))), "0.3.1").is_some(), "{status}");
    }
    assert_eq!(pin_pending_for(Some(&row("running", Some("0.3.1"))), "0.3.1"), None, "current");
    assert_eq!(pin_pending_for(Some(&row("running", Some("0.4.0"))), "0.3.1"), None, "newer than the pin");
    assert_eq!(pin_pending_for(Some(&row("installing", Some("0.3.0"))), "0.3.1"), None, "enable owns it");
    assert_eq!(pin_pending_for(Some(&row("removed", Some("0.3.0"))), "0.3.1"), None, "not installed");
    assert_eq!(pin_pending_for(Some(&row("running", None)), "0.3.1"), None, "no installed version recorded");
    assert_eq!(pin_pending_for(None, "0.3.1"), None, "no Sew on this box");
}

/// T6 + Rosson Q4/Q16: an App that can't run two copies (no measured room,
/// extra ports, the process exception, handover) pauses briefly and updates
/// by default; with "skip and let me update it myself" it is deferred with
/// a card; `--pause` forces it.
#[test]
fn no_overlap_pauses_by_default_and_defers_on_request() {
    let _g = serial();
    let mut f = Fake::default();
    let (room, ports, skip_room, skip_ports) = ("w6-room", "w6-ports", "w6-skip-room", "w6-skip-ports");
    f.apps = vec![
        row(room),
        row(ports),
        AppRow { rehost_when_no_overlap: "defer".into(), ..row(skip_room) },
        AppRow { rehost_when_no_overlap: "defer".into(), ..row(skip_ports) },
    ];
    f.no_room = vec![room.into(), skip_room.into()];
    f.shape.insert(ports.into(), "it holds extra ports (5000)".into());
    f.shape.insert(skip_ports.into(), "it holds extra ports (5000)".into());
    let turns: HashMap<String, Turn> = run_wave(&f).into_iter().collect();
    assert!(matches!(turns[room], Turn::Done(_)) && matches!(turns[ports], Turn::Done(_)));
    let r = f.rehosted.borrow().clone();
    assert!(r.contains(&(room.into(), true)) && r.contains(&(ports.into(), true)), "a pause, then the update: {r:?}");
    match &turns[skip_room] {
        Turn::Deferred(code, detail) => assert!(*code == defer::NO_HEADROOM && detail.contains("900 MiB"), "the numbers: {detail}"),
        t => panic!("{t:?}"),
    }
    assert!(matches!(turns[skip_ports], Turn::Deferred(defer::NEEDS_HANDOVER, _)));
    assert_eq!(f.owner.borrow().len(), 2, "one card each: {:?}", f.owner.borrow());
    assert!(matches!(turn(&f, &row(skip_ports), Some(Forced { yes: false, pause: true })), Turn::Done(_)));
}

/// T7: policies. off/manual never move by themselves, window only inside
/// it, the box-wide switch stops all; `k2 app rehost` moves any of them.
#[test]
fn policies_gate_the_wave() {
    let _g = serial();
    let mut f = Fake::default();
    let (off, manual, win, out) = ("w7-off", "w7-manual", "w7-win", "w7-out");
    f.apps = vec![
        AppRow { rehost_policy: "off".into(), ..row(off) },
        AppRow { rehost_policy: "manual".into(), ..row(manual) },
        AppRow { rehost_policy: "window".into(), rehost_window: Some("02:00-05:00".into()), ..row(win) },
        AppRow { rehost_policy: "window".into(), rehost_window: Some("10:00-11:00".into()), ..row(out) },
    ];
    f.minute = 3 * 60;
    let turns: HashMap<String, Turn> = run_wave(&f).into_iter().collect();
    assert!(matches!(turns[off], Turn::Deferred(defer::POLICY_OFF, _)));
    assert!(matches!(turns[manual], Turn::Deferred(defer::POLICY_MANUAL, _)));
    assert!(matches!(turns[win], Turn::Done(_)));
    assert!(matches!(turns[out], Turn::Deferred(defer::OUTSIDE_WINDOW, _)));
    assert!(matches!(turn(&f, &f.apps[0], Some(Forced::default())), Turn::Done(_)), "k2 app rehost moves an off App");
    let g = Fake { apps: vec![row("w7-env")], off: true, ..Default::default() };
    assert!(matches!(run_wave(&g)[0].1, Turn::Deferred(defer::POLICY_OFF, _)), "K2_APPS_REHOST=off");
}

/// T32 (A13): two failed re-hosts in a row stop the wave with one owner
/// notice; a deferral never counts; a failed App is held (`rehost_failed`)
/// until the owner asks again, and its agent hears once.
#[test]
fn the_breaker_counts_failures_not_deferrals() {
    let _g = serial();
    let mut f = Fake::default();
    let (d1, f1, d2, f2, never) = ("w32-a-defer", "w32-b-fail", "w32-c-defer", "w32-d-fail", "w32-e-never");
    f.apps = vec![row(d1), row(f1), row(d2), row(f2), row(never)];
    f.requires_bad = vec![d1.into(), d2.into()];
    f.fail = vec![f1.into(), f2.into()];
    let order = |a: &str| -> usize { [d1, f1, d2, f2, never].iter().position(|x| *x == a).unwrap() };
    for a in [d1, f1, d2, f2, never] {
        f.conns.insert(a.into(), order(a));
    }
    let turns = run_wave(&f);
    assert_eq!(turns.len(), 4, "stopped after the second failure: {turns:?}");
    assert!(f.rehosted.borrow().iter().all(|(a, _)| a != never), "the rest of the wave never ran");
    assert_eq!(status_json()["state"], "stopped");
    assert_eq!(f.owner.borrow().iter().filter(|t| t.contains("stopped moving Apps")).count(), 1);
    assert!(events(&f, f1).contains(&"rehost_failed".to_string()));
    assert_eq!(f.agent.borrow().iter().filter(|(a, _)| a == f1).count(), 1, "the agent hears its failed re-host once");
    // Held: the next wave on this version defers it.
    let g = Fake { apps: vec![row(f1)], held: vec![f1.into()], ..Default::default() };
    assert!(matches!(run_wave(&g)[0].1, Turn::Deferred(defer::REHOST_FAILED, _)));
}

/// R14/R15/R16 + Rosson Q4/Q16, the routes: the App's block reads `pause`
/// by default; the owner sets `defer`, a policy and a window; bad values
/// and a window policy without a window are refused; a Member login reads
/// and changes nothing.
#[test]
fn the_policy_route_validates_and_the_status_route_reads() {
    let _g = serial();
    let _home = crate::test_support::TempHome::new();
    k2_core::deploy::set_enabled(true);
    let id = "w-route-app";
    {
        let h = k2_core::db::shared();
        let c = h.lock();
        c.execute("DELETE FROM apps WHERE app_id = ?1", rusqlite::params![id]).unwrap();
        store::insert_app(
            &c,
            &store::NewApp {
                app_id: id,
                service_id: "svc-w-route-app",
                seed_id: None,
                owner_workspace: "ws-w-route",
                isolation: "container",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: "owner",
            },
        )
        .unwrap();
    }
    let call = |path: &str, who: super::super::routes::Who, body: &str, q: &[(&str, &str)]| -> (u16, J) {
        let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let r = super::super::routes::handle(path, who, body.as_bytes(), &p);
        let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
        (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body)))
    };
    use super::super::routes::Who;
    let (n, v) = call("/cli/app/rehost", Who::Owner, "", &[("app", id)]);
    assert_eq!(n, 200, "{v}");
    assert_eq!((v["rehost"]["whenNoOverlap"].as_str(), v["rehost"]["policy"].as_str()), (Some("pause"), Some("auto")), "{v}");
    let (n, v) = call("/cli/app/rehost/policy", Who::Owner, &format!(r#"{{"app":"{id}","whenNoOverlap":"defer"}}"#), &[]);
    assert_eq!((n, v["rehost"]["whenNoOverlap"].as_str()), (200, Some("defer")), "{v}");
    for bad in [
        format!(r#"{{"app":"{id}","whenNoOverlap":"later"}}"#),
        format!(r#"{{"app":"{id}","policy":"sometimes"}}"#),
        format!(r#"{{"app":"{id}","policy":"window"}}"#),
        format!(r#"{{"app":"{id}","window":"25:00-26:00"}}"#),
        format!(r#"{{"app":"{id}"}}"#),
    ] {
        let (n, v) = call("/cli/app/rehost/policy", Who::Owner, &bad, &[]);
        assert_eq!(n, 400, "{bad}: {v}");
    }
    let (n, v) = call("/cli/app/rehost/policy", Who::Owner, &format!(r#"{{"app":"{id}","policy":"window","window":"02:00-05:00"}}"#), &[]);
    assert_eq!((n, v["rehost"]["policy"].as_str(), v["rehost"]["window"].as_str()), (200, Some("window"), Some("02:00-05:00")), "{v}");
    let member = || Who::Login { username: "mia".into(), role: k2_core::connect_users::Role::Member };
    let (n, _) = call("/cli/app/rehost", member(), "", &[]);
    assert_eq!(n, 200, "a Member reads the wave");
    let (n, v) = call("/cli/app/rehost/policy", member(), &format!(r#"{{"app":"{id}","whenNoOverlap":"pause"}}"#), &[]);
    assert_eq!(n, 403, "a Member changes nothing: {v}");
    let (n, v) = call("/cli/app/rehost/run", member(), &format!(r#"{{"app":"{id}"}}"#), &[]);
    assert_eq!(n, 403, "{v}");
}

/// `k2 app rehost` printed "None moved to a fresh instance": the Done and
/// Failed answers now name the App and how it moved, and the status keeps
/// the last run's mode (`overlap` | `pause`) and whether it was asked for,
/// across later states, until the next run.
#[test]
fn a_run_names_the_app_and_its_mode() {
    let _g = serial();
    let mut f = Fake::default();
    let (wave, asked, failed) = ("wm-wave", "wm-asked", "wm-failed");
    f.apps = vec![row(wave)];
    assert!(matches!(run_wave(&f)[0].1, Turn::Done(_)));
    let st = status_json()["apps"][wave].clone();
    assert_eq!(st["state"], "done");
    assert_eq!((st["mode"].as_str(), st["forced"].as_bool()), (Some("overlap"), Some(false)), "the wave's own turn: {st}");

    f.shape.insert(asked.into(), "it holds extra ports (5000)".into());
    assert!(matches!(turn(&f, &row(asked), Some(Forced { yes: false, pause: true })), Turn::Done(_)));
    let st = status_json()["apps"][asked].clone();
    assert_eq!((st["mode"].as_str(), st["forced"].as_bool()), (Some("pause"), Some(true)), "{st}");
    let deployment = json!({ "ok": true, "deployment": { "id": "d-new-wm-asked" }, "done": true });
    let out = run_answer(deployment.clone(), asked, "done", None);
    assert_eq!(out["app"], asked, "{out}");
    assert_eq!((out["rehost"].as_str(), out["mode"].as_str(), out["forced"].as_bool()), (Some("done"), Some("pause"), Some(true)), "{out}");
    assert_eq!(out["deployment"]["id"], "d-new-wm-asked", "the deployment row stays");
    assert!(out.get("why").is_none(), "{out}");

    f.fail = vec![failed.into()];
    assert!(matches!(turn(&f, &row(failed), Some(Forced::default())), Turn::Failed(..)));
    let st = status_json()["apps"][failed].clone();
    assert_eq!(st["state"], "failed");
    assert_eq!((st["mode"].as_str(), st["forced"].as_bool()), (Some("overlap"), Some(true)), "{st}");
    let out = run_answer(deployment, failed, "failed", Some("health_failed"));
    assert_eq!((out["app"].as_str(), out["mode"].as_str(), out["why"].as_str()), (Some(failed), Some("overlap"), Some("health_failed")), "{out}");

    // A later deferral keeps the last run's words.
    f.busy = vec![asked.into()];
    assert!(matches!(turn(&f, &row(asked), None), Turn::Deferred(defer::BUSY, _)));
    let st = status_json()["apps"][asked].clone();
    assert_eq!(st["state"], "deferred");
    assert_eq!((st["mode"].as_str(), st["forced"].as_bool()), (Some("pause"), Some(true)), "{st}");

    // Never ran here: no mode, and the answer says so (null).
    assert_eq!(last_mode("wm-never"), None);
    assert!(run_answer(json!({}), "wm-never", "done", None)["mode"].is_null());
}
