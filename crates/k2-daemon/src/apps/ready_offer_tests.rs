use super::*;
use std::cell::{Cell, RefCell};

/// A fake box: the doctor before and after, what each step answers, and
/// every call made.
struct Fake {
    linux: bool,
    apps: bool,
    /// Doctors in order (the last one repeats).
    doctors: RefCell<Vec<J>>,
    mem: Option<u64>,
    latch_held: bool,
    prepare: Result<J, String>,
    engine: Result<J, String>,
    restarter: bool,
    zone: Option<String>,
    guests: (Vec<(String, u64)>, Vec<(String, u64)>),
    now: Cell<i64>,
    calls: RefCell<Vec<String>>,
    prepared_with: RefCell<Vec<(PrepareOpts, PodmanOpts)>>,
    held: Cell<i32>,
}

fn check(id: &str, status: &str) -> J {
    json!({ "id": id, "status": status, "detail": format!("{id} detail"), "fix": "k2 app box prepare (owner)" })
}

fn not_ready() -> J {
    json!({
        "ready": false,
        "blocking": ["nft", "appUser"],
        "checks": [check("kernel", "ok"), check("nft", "fail"), check("appUser", "fail"), check("coresSlice", "warn"),
                   check("agentsLeaf", "warn"), check("engine", "fail")],
    })
}

/// Prepared, the cores drop-in in place, the daemon not restarted yet.
fn ready_needs_restart() -> J {
    json!({
        "ready": true,
        "blocking": [],
        "checks": [check("kernel", "ok"), check("nft", "ok"), check("appUser", "ok"), check("coresSlice", "ok"),
                   check("agentsLeaf", "warn"), check("engine", "ok")],
    })
}

fn ready_all() -> J {
    json!({
        "ready": true,
        "blocking": [],
        "checks": [check("kernel", "ok"), check("coresSlice", "ok"), check("agentsLeaf", "ok"), check("engine", "ok")],
    })
}

impl Fake {
    fn new() -> Self {
        Fake {
            linux: true,
            apps: true,
            doctors: RefCell::new(vec![not_ready()]),
            mem: Some(65_842_476 * 1024),
            latch_held: false,
            prepare: Ok(json!({"ok": true, "steps": [{"verb": "apps ensure-user", "ok": true, "result": {"changed": true}}]})),
            engine: Ok(json!({"engine": {"version": "x"}})),
            restarter: true,
            zone: None,
            guests: (Vec::new(), Vec::new()),
            now: Cell::new(1_800_000_000),
            calls: RefCell::new(Vec::new()),
            prepared_with: RefCell::new(Vec::new()),
            held: Cell::new(0),
        }
    }
    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
    fn restarts(&self) -> usize {
        self.calls().iter().filter(|c| *c == "restart_now").count()
    }
}

impl Ops for Fake {
    fn linux(&self) -> bool {
        self.linux
    }
    fn apps_on(&self) -> bool {
        self.apps
    }
    fn doctor(&self) -> J {
        let mut d = self.doctors.borrow_mut();
        if d.len() > 1 {
            d.remove(0)
        } else {
            d[0].clone()
        }
    }
    fn mem_total_bytes(&self) -> Option<u64> {
        self.mem
    }
    fn begin_busy(&self) -> bool {
        if self.latch_held {
            return false;
        }
        self.held.set(self.held.get() + 1);
        true
    }
    fn end_busy(&self) {
        self.held.set(self.held.get() - 1);
    }
    fn prepare(&self, opts: &PrepareOpts, pod: &PodmanOpts) -> Result<J, String> {
        self.calls.borrow_mut().push("prepare".into());
        self.prepared_with.borrow_mut().push((opts.clone(), pod.clone()));
        self.prepare.clone()
    }
    fn install_engine(&self) -> Result<J, String> {
        self.calls.borrow_mut().push("engine".into());
        self.engine.clone()
    }
    fn restart_now(&self) -> Result<(), String> {
        self.calls.borrow_mut().push("restart_now".into());
        if self.restarter {
            Ok(())
        } else {
            Err("no restarter".into())
        }
    }
    fn restart_schedule_changed(&self) {
        self.calls.borrow_mut().push("schedule_changed".into());
    }
    fn owner_zone(&self) -> Option<String> {
        self.zone.clone()
    }
    fn guests(&self) -> (Vec<(String, u64)>, Vec<(String, u64)>) {
        self.guests.clone()
    }
    fn now(&self) -> i64 {
        self.now.get()
    }
    fn audit(&self, event: &str, _actor: &str, detail: &str) {
        self.calls.borrow_mut().push(format!("audit {event} {detail}"));
    }
}

fn store() -> (crate::test_support::Scratch, Store) {
    let dir = crate::test_support::Scratch::new();
    let path = dir.path().join("apps-ready-offer.json");
    (dir, Store { path })
}

fn owner_login() -> Who {
    Who::Login { username: "olive".into(), role: Role::Owner }
}

fn not_owners() -> Vec<Who> {
    vec![
        Who::Login { username: "ada".into(), role: Role::Admin },
        Who::Login { username: "max".into(), role: Role::Member },
        Who::Agent { project_id: "p".into(), session_id: "s".into() },
        Who::Shell { project_id: "p".into(), session_id: "s".into() },
    ]
}

/// The offer needs Linux, Apps on, a NOT READY doctor, no run going, and a
/// waiting trigger.
#[test]
fn the_offer_conditions() {
    let mut s = Saved::default();
    let f = Facts { linux: true, apps_on: true, ready: false };
    assert!(offer(&f, &s).is_none(), "no trigger, no card");
    note_trigger(&mut s, TRIGGER_UPGRADE, 10);
    let o = offer(&f, &s).expect("offered");
    assert_eq!(o["trigger"], TRIGGER_UPGRADE);
    assert_eq!(o["title"], TITLE);
    let line = o["line"].as_str().unwrap();
    assert!(line.contains("Podman") && line.contains("App user") && line.contains("firewall") && line.contains("No restart now"), "{line}");
    for (what, f) in [
        ("macOS", Facts { linux: false, ..f.clone() }),
        ("Apps off", Facts { apps_on: false, ..f.clone() }),
        ("ready", Facts { ready: true, ..f.clone() }),
    ] {
        assert!(offer(&f, &s).is_none(), "{what}: no card");
    }
    s.run = Some(Run { state: "running".into(), ..Run::default() });
    assert!(offer(&f, &s).is_none(), "not while a run is going");
    s.run = Some(Run { state: "failed".into(), ..Run::default() });
    assert!(offer(&f, &s).is_some(), "a failed run leaves the card");
}

/// Each trigger is offered once: answered, it never comes back; a new
/// trigger is a new offer; the same trigger again is nothing new.
#[test]
fn once_per_trigger() {
    let mut s = Saved::default();
    let f = Facts { linux: true, apps_on: true, ready: false };
    on_boot(&mut s, 1);
    assert_eq!(pending(&s).map(|t| t.key.as_str()), Some(TRIGGER_UPGRADE));
    on_boot(&mut s, 2);
    assert_eq!(s.triggers.len(), 1, "the upgrade trigger is once per server, not every boot");
    assert_eq!(answer(&mut s, Some(TRIGGER_UPGRADE), "dismissed", 3), 1);
    assert!(offer(&f, &s).is_none(), "answered: gone");
    on_boot(&mut s, 4);
    assert!(offer(&f, &s).is_none(), "still gone after a restart");
    assert!(note_trigger(&mut s, TRIGGER_APPS_PAGE, 5));
    assert_eq!(offer(&f, &s).unwrap()["trigger"], TRIGGER_APPS_PAGE);
    assert!(!note_trigger(&mut s, TRIGGER_APPS_PAGE, 6), "opening the page again is not a new trigger");
    answer(&mut s, Some(TRIGGER_APPS_PAGE), "dismissed", 7);
    assert!(!note_trigger(&mut s, TRIGGER_APPS_PAGE, 8));
    assert!(offer(&f, &s).is_none(), "never again for that trigger");
}

/// A paid plan starting is a trigger; the first reading only records the
/// tier; staying paid or moving between paid plans is not a start.
#[test]
fn a_paid_plan_starting_is_a_trigger() {
    let mut s = Saved::default();
    assert_eq!(on_tier(&mut s, Some("pro"), 1), None, "first reading: recorded only");
    assert_eq!(s.last_tier.as_deref(), Some("pro"));
    assert_eq!(on_tier(&mut s, Some("single"), 2), None, "paid → paid");
    assert_eq!(on_tier(&mut s, Some("free"), 3), None);
    assert_eq!(on_tier(&mut s, Some("single"), 4).as_deref(), Some("subscription:single"));
    assert_eq!(on_tier(&mut s, None, 5), None);
    assert_eq!(on_tier(&mut s, Some("Pro"), 6).as_deref(), Some("subscription:pro"), "empty → paid");
    on_tier(&mut s, Some("free"), 7);
    assert_eq!(on_tier(&mut s, Some("pro"), 8), None, "the same plan starting again: offered once");
    let (_d, st) = store();
    tier_with(&st, Some("free"), 1).unwrap();
    assert_eq!(tier_with(&st, Some("single"), 2).unwrap().as_deref(), Some("subscription:single"), "kept across restarts");
}

/// Owner logins only: Admin, Member, agents and shells get no card and
/// can't answer, run, restart or read live sessions.
#[test]
fn owner_only() {
    let f = Fake::new();
    let (_d, st) = store();
    boot_with(&st, 1).unwrap();
    for w in [Who::Owner, owner_login()] {
        let v = status_with(&f, &st, &w).unwrap();
        assert_eq!(v["owner"], true, "{w:?}");
        assert_eq!(v["offer"]["trigger"], TRIGGER_UPGRADE, "{w:?}: {v}");
    }
    for w in not_owners() {
        let v = status_with(&f, &st, &w).unwrap();
        assert_eq!(v, json!({"owner": false, "offer": null}), "{w:?}");
        let v = trigger_with(&f, &st, &w, &json!({"trigger": "apps_page"})).unwrap();
        assert_eq!(v["recorded"], false, "{w:?}");
        assert_eq!(dismiss_with(&f, &st, &w, &json!({"trigger": "upgrade"})).unwrap_err().status, 403, "{w:?}");
        assert_eq!(begin_run(&f, &st, &w).unwrap_err().status, 403, "{w:?}");
        assert_eq!(restart_with(&f, &st, &w, &json!({"when": "now"})).unwrap_err().status, 403, "{w:?}");
        assert_eq!(restart_with(&f, &st, &w, &json!({"when": "tonight"})).unwrap_err().status, 403, "{w:?}");
        assert_eq!(sessions_live_with(&f, &w).unwrap_err().status, 403, "{w:?}");
    }
    assert!(f.calls().iter().all(|c| !c.starts_with("prepare") && c != "restart_now"), "{:?}", f.calls());
    assert!(st.load().triggers.iter().all(|t| t.key != TRIGGER_APPS_PAGE), "an Admin opening the page is no trigger");
    for r in ROUTES.iter().chain(POST_ROUTES) {
        assert!(!crate::session_token::is_agent_verb(r), "no passport reaches {r}");
    }
}

/// The Apps page trigger and "Not now" through the routes.
#[test]
fn apps_page_and_not_now() {
    let f = Fake::new();
    let (_d, st) = store();
    let v = trigger_with(&f, &st, &Who::Owner, &json!({"trigger": "apps_page"})).unwrap();
    assert_eq!(v["recorded"], true);
    assert_eq!(v["offer"]["trigger"], TRIGGER_APPS_PAGE);
    assert_eq!(trigger_with(&f, &st, &Who::Owner, &json!({"trigger": "upgrade"})).unwrap_err().status, 400, "daemon-owned");
    let v = dismiss_with(&f, &st, &Who::Owner, &json!({"trigger": "apps_page"})).unwrap();
    assert!(v["offer"].is_null(), "{v}");
    assert_eq!(dismiss_with(&f, &st, &Who::Owner, &json!({"trigger": "apps_page"})).unwrap_err().status, 404);
    assert_eq!(dismiss_with(&f, &st, &Who::Owner, &json!({})).unwrap_err().status, 400);
    let v = trigger_with(&f, &st, &Who::Owner, &json!({"trigger": "apps_page"})).unwrap();
    assert_eq!(v["recorded"], false);
    assert!(v["offer"].is_null(), "offered once");
    assert!(f.calls().iter().all(|c| c != "prepare"), "never automatic");
}

/// The run goes through the owner's prepare path with --install-podman and
/// the cores floor sized by RAM (4 GiB on a 64 GB box), then the engine,
/// then the doctor; it answers the waiting triggers and releases the latch.
#[test]
fn the_run_prepares_through_the_existing_path_with_the_right_flags() {
    let f = Fake::new();
    *f.doctors.borrow_mut() = vec![not_ready(), not_ready(), ready_needs_restart()];
    let (_d, st) = store();
    boot_with(&st, 1).unwrap();
    let by = begin_run(&f, &st, &owner_login()).unwrap();
    assert_eq!(f.held.get(), 1, "the root latch");
    assert_eq!(st.load().run.unwrap().state, "running");
    assert!(offer(&Facts { linux: true, apps_on: true, ready: false }, &st.load()).is_none());
    assert_eq!(begin_run(&f, &st, &Who::Owner).unwrap_err().code, "already_running");
    run_job(&f, &st, &by);
    assert_eq!(f.held.get(), 0, "released");
    let (opts, pod) = f.prepared_with.borrow()[0].clone();
    assert_eq!(opts, PrepareOpts { runtime: k2_root_proto::apps::OciRuntime::Crun, cores_mem_min_mib: Some(4096), budget: None });
    assert_eq!(pod, PodmanOpts { install: true, dry_run: false, from: None }, "k2 app box prepare --install-podman");
    assert_eq!(f.calls().iter().filter(|c| !c.starts_with("audit")).cloned().collect::<Vec<_>>(), vec!["prepare", "engine"]);
    let r = st.load().run.unwrap();
    assert_eq!((r.state.as_str(), r.phase.as_str(), r.ready), ("done", "finished", Some(true)), "{r:?}");
    assert_eq!(r.cores_mem_min_mib, Some(4096));
    assert!(r.changed.contains(&"apps ensure-user".to_string()) && r.changed.contains(&"installed the App engine".to_string()), "{r:?}");
    assert!(r.restart_needed, "the agents leaf waits for a restart");
    assert!(st.load().triggers.iter().all(|t| t.answer.as_deref() == Some("ran")), "run answers the offer");
}

/// The restart is deferred: the run never restarts; the card offers it,
/// with how many guests a restart would sign out.
#[test]
fn the_restart_is_deferred_not_immediate() {
    let mut f = Fake::new();
    *f.doctors.borrow_mut() = vec![not_ready(), not_ready(), ready_needs_restart()];
    f.guests = (vec![("shop".into(), 2)], vec![("host".into(), 1), ("shop".into(), 5)]);
    let (_d, st) = store();
    let by = begin_run(&f, &st, &Who::Owner).unwrap();
    run_job(&f, &st, &by);
    assert_eq!(f.restarts(), 0, "no restart from the run");
    let v = status_with(&f, &st, &Who::Owner).unwrap();
    assert_eq!(v["restartNeeded"], true, "{v}");
    assert!(v["restart"].is_null());
    assert_eq!(v["guests"]["connected"], 2, "{v}");
    assert_eq!(v["guests"]["signedIn"], 6, "{v}");
    assert_eq!(v["guests"]["apps"], json!([{"app": "host", "connected": 0, "signedIn": 1}, {"app": "shop", "connected": 2, "signedIn": 5}]));
    // Tonight at 3 am in the owner's zone (the app sends it).
    f.now.set(1_800_000_000); // 2027-01-15T08:00:00Z = 01:00 in America/Denver
    let v = restart_with(&f, &st, &Who::Owner, &json!({"when": "tonight", "tz": "America/Denver"})).unwrap();
    assert_eq!(f.restarts(), 0, "scheduled, not restarted");
    let want = 1_800_000_000 + 2 * 3600;
    assert_eq!(v["restart"]["at"], want, "{v}");
    assert_eq!(v["restart"]["zone"], "America/Denver");
    assert!(f.calls().contains(&"schedule_changed".to_string()));
    // The owner's setting when the app sends none.
    f.zone = Some("Europe/Berlin".into());
    let v = restart_with(&f, &st, &Who::Owner, &json!({"when": "tonight"})).unwrap();
    assert_eq!(v["restart"]["zone"], "Europe/Berlin");
    assert_eq!(f.restarts(), 0);
    assert_eq!(restart_with(&f, &st, &Who::Owner, &json!({"when": "tonight", "tz": "Mars/Base"})).unwrap_err().status, 400);
    let v = restart_with(&f, &st, &Who::Owner, &json!({"when": "cancel"})).unwrap();
    assert!(v["restart"].is_null());
    assert_eq!(restart_with(&f, &st, &Who::Owner, &json!({"when": "later"})).unwrap_err().status, 400);
    // Only the owner's "Restart now" restarts.
    restart_with(&f, &st, &Who::Owner, &json!({"when": "now"})).unwrap();
    assert_eq!(f.restarts(), 1);
    // A start after the schedule means it happened.
    restart_with(&f, &st, &Who::Owner, &json!({"when": "tonight", "tz": "UTC"})).unwrap();
    boot_with(&st, f.now.get() + 10).unwrap();
    assert!(st.load().restart.is_none(), "a boot clears a done restart");
}

/// 3 am is the next one: later today when it's before 3, else tomorrow.
#[test]
fn tonight_is_the_next_3_am_in_the_zone() {
    // 2027-01-15T08:00:00Z: 01:00 in Denver (MST, UTC-7), 09:00 in Berlin.
    let now = 1_800_000_000;
    assert_eq!(next_local_hour(now, Some("America/Denver"), 3).unwrap().0, now + 2 * 3600);
    assert_eq!(next_local_hour(now, Some("Europe/Berlin"), 3).unwrap().0, now + 18 * 3600, "tomorrow 03:00 CET");
    assert_eq!(next_local_hour(now, Some("UTC"), 3).unwrap(), (now + 19 * 3600, "UTC".to_string()));
    assert!(next_local_hour(now, Some("nowhere"), 3).is_err());
    let (t, _) = next_local_hour(now, None, 3).unwrap();
    assert!(t > now && t <= now + 25 * 3600, "the box clock: within a day");
}

/// A failed step is recorded with the doctor's blocking rows; the latch is
/// released; a held latch refuses before any change.
#[test]
fn a_failure_is_recorded_and_the_latch_released() {
    let mut f = Fake::new();
    f.prepare = Err("box prep stopped at `apps nft apply`: no nft".into());
    let (_d, st) = store();
    let by = begin_run(&f, &st, &Who::Owner).unwrap();
    run_job(&f, &st, &by);
    assert_eq!(f.held.get(), 0);
    let r = st.load().run.unwrap();
    assert_eq!((r.state.as_str(), r.phase.as_str()), ("failed", "prepare"));
    assert!(r.error.unwrap().contains("apps nft apply"));
    let ids: Vec<&str> = r.blocking.iter().map(|b| b["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["nft", "appUser"]);
    assert!(!f.calls().contains(&"engine".to_string()));
    let mut g = Fake::new();
    g.latch_held = true;
    let (_d2, st2) = store();
    assert_eq!(begin_run(&g, &st2, &Who::Owner).unwrap_err().code, "root_busy");
    assert!(st2.load().run.is_none());
    let mut m = Fake::new();
    m.linux = false;
    assert_eq!(begin_run(&m, &st2, &Who::Owner).unwrap_err().code, "linux_only");
}

/// A box that has a cores floor keeps the owner's (no --cores-mem-min);
/// an engine already answering isn't installed again; a run cut off by a
/// restart reads as failed after the boot.
#[test]
fn a_cores_floor_and_engine_already_there_are_kept() {
    let f = Fake::new();
    *f.doctors.borrow_mut() = vec![ready_needs_restart(), ready_all()];
    let (_d, st) = store();
    let by = begin_run(&f, &st, &Who::Owner).unwrap();
    run_job(&f, &st, &by);
    assert_eq!(f.prepared_with.borrow()[0].0.cores_mem_min_mib, None);
    assert!(!f.calls().contains(&"engine".to_string()));
    assert!(!st.load().run.unwrap().restart_needed);
    begin_run(&f, &st, &Who::Owner).unwrap();
    boot_with(&st, 99).unwrap();
    let r = st.load().run.unwrap();
    assert_eq!(r.state, "failed", "{r:?}");
}

/// The doctor's blocking rows and the restart rule.
#[test]
fn blocking_rows_and_the_restart_rule() {
    let ids = |d: &J| blocking_rows(d).iter().map(|b| b["id"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(ids(&not_ready()), vec!["nft", "appUser"]);
    let mut old = not_ready();
    old.as_object_mut().unwrap().remove("blocking");
    assert_eq!(ids(&old), vec!["nft", "appUser", "engine"], "an older daemon: every row not ok or warn");
    assert!(restart_needed(&ready_needs_restart()));
    assert!(!restart_needed(&ready_all()));
    assert!(!restart_needed(&not_ready()), "no cores drop-in yet: prepare first");
}

/// The live sessions read: counts per App, totals, no names.
#[test]
fn live_sessions_are_counts() {
    let mut f = Fake::new();
    f.guests = (vec![("host".into(), 1)], vec![("host".into(), 3), ("svc-x".into(), 2)]);
    let v = sessions_live_with(&f, &Who::Owner).unwrap();
    assert_eq!(v, json!({"connected": 1, "signedIn": 5, "apps": [
        {"app": "host", "connected": 1, "signedIn": 3}, {"app": "svc-x", "connected": 0, "signedIn": 2}]}));
}
