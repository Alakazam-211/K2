//! The manager scenario on a fake lane (lane-blind behaviour: limits,
//! snapshots, diff, hold, audit, reaper, adopt).

use super::*;
use crate::sim::fake::FakeLane;
use std::sync::atomic::Ordering;

fn ws(id: &str, path: &Path) -> Ws {
    Ws { id: id.to_string(), handle: format!("h-{id}"), path: path.to_path_buf() }
}

fn person() -> Actor {
    Actor::Person { name: "owner".into() }
}

fn agent() -> Actor {
    Actor::Agent { handle: "one".into() }
}

fn tap(element: &str) -> ActReq {
    ActReq { workspace: None, action: ActSpec::Tap { element: Some(element.into()), x: None, y: None, hold_ms: None }, no_diff: false }
}

fn save_id(t: &TreeResp) -> String {
    t.snapshot.elements.iter().find(|e| e.label.as_deref() == Some("Save")).expect("a Save button").id.clone()
}

/// The lane registry and the sim home are process-wide: tests that use
/// them run one at a time.
fn sim_lock() -> std::sync::MutexGuard<'static, ()> {
    static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
    L.lock().unwrap_or_else(|e| e.into_inner())
}

/// One sequential scenario: the lane registry and the sim home are
/// process-wide.
#[test]
fn manager_scenario_with_a_fake_lane() {
    let _serial = sim_lock();
    let tmp = std::env::temp_dir().join(format!("k2-sim-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("a")).expect("ws a");
    std::fs::create_dir_all(tmp.join("b")).expect("ws b");
    crate::sim::set_base_for_tests(tmp.join("home"));
    limits::save_settings(&limits::SimSettings { max_devices: Some(1), min_free_ram_bytes: Some(0), ..Default::default() }).expect("limits");
    let fake = Arc::new(FakeLane::new(LaneKind::Android));
    crate::sim::register_lane(fake.clone());
    {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT OR REPLACE INTO projects (id, name, path) VALUES ('p-sim-a', 'a', ?1), ('p-sim-b', 'b', ?2)",
            rusqlite::params![tmp.join("a").display().to_string(), tmp.join("b").display().to_string()],
        )
        .expect("projects");
    }
    let a = ws("p-sim-a", &tmp.join("a"));
    let b = ws("p-sim-b", &tmp.join("b"));
    let m = Manager::new();

    // Status before anything runs.
    let st = m.status(&a, true).expect("status");
    assert!(!st.agents_allowed, "the switch is off by default");
    assert!(st.sim.is_none());
    assert!(st.lanes.iter().any(|p| p.lane == LaneKind::Android && p.usable));
    assert!(m.any_lane_usable());

    // Setup pending → exit 3 naming the owner's card.
    fake.ready.store(false, Ordering::SeqCst);
    m.refresh_probes();
    let e = m.boot(&a, &person(), &BootReq::default()).expect_err("needs setup");
    assert_eq!((e.code, e.exit, e.status), ("sim_needs_setup", 3, 409));
    assert!(!e.needs.is_empty());
    fake.ready.store(true, Ordering::SeqCst);
    m.refresh_probes();

    // Boot; a second workspace hits the limit of 1.
    let v = m.boot(&a, &person(), &BootReq::default()).expect("boot a");
    assert_eq!(v.state, SimState::Running);
    assert_eq!(v.profile.as_ref().map(|p| p.name.as_str()), Some("Fake Phone"));
    assert!(v.sim.starts_with('s'));
    let again = m.boot(&a, &person(), &BootReq::default()).expect("boot a again is a no-op");
    assert_eq!(again.sim, v.sim);
    assert_eq!(fake.boots.load(Ordering::SeqCst), 1, "no second boot");
    let e = m.boot(&b, &person(), &BootReq::default()).expect_err("limit");
    assert_eq!(e.code, "sim_limit");
    assert!(e.message.contains("1 of its 1 devices"), "{}", e.message);

    // Element ids need a snapshot; then tap → diff.
    let e = m.act(&a, &agent(), &tap("e3"), None).expect_err("no snapshot");
    assert_eq!(e.code, "sim_stale_element");
    let t = m.tree(&a, &agent(), false, None, false).expect("tree");
    assert!(t.text.contains("button \"Save\""), "{}", t.text);
    assert!(t.text.contains("· snapshot t1 ·"), "{}", t.text);
    let save = save_id(&t);
    let r = m.act(&a, &agent(), &tap(&save), None).expect("tap save");
    assert!(r.line.starts_with(&format!("ok tap {save} · ")), "{}", r.line);
    assert!(r.line.contains("text \"Saved\""), "{}", r.line);
    assert_eq!(r.snapshot.as_deref(), Some("t2"));
    let e = m.act(&a, &agent(), &tap(&format!("t1:{save}")), None).expect_err("stale id");
    assert_eq!(e.code, "sim_stale_element");
    assert!(e.message.contains("snapshot t1") && e.message.contains("t2"), "{}", e.message);

    // Typing: the value never shows in the line or the lane's log.
    let ty = ActReq { workspace: None, action: ActSpec::Type { into: None, text: Some("a@b.c".into()), secret: None }, no_diff: true };
    let r = m.act(&a, &agent(), &ty, None).expect("type");
    assert_eq!(r.line.split(" · ").next(), Some("ok type 5 chars"));
    let sec = ActReq { workspace: None, action: ActSpec::Type { into: None, text: None, secret: Some("pw".into()) }, no_diff: true };
    let e = m.act(&a, &agent(), &sec, None).expect_err("an unresolved secret fails");
    assert_eq!(e.code, "sim_invalid");
    let r = m.act(&a, &agent(), &sec, Some(SimText::secret("hunter2"))).expect("secret");
    assert!(!r.line.contains("hunter2"), "{}", r.line);
    assert!(fake.actions.lock().iter().all(|l| !l.contains("hunter2")), "the secret reached a log line");

    // Person wins: take over → agent held (exit 2); hand back → read tree first.
    let e = m.hold(&a, &agent(), HoldAction::TakeOver).expect_err("agents can't take over");
    assert_eq!(e.code, "forbidden");
    m.hold(&a, &person(), HoldAction::TakeOver).expect("take over");
    let e = m.act(&a, &agent(), &tap(&save), None).expect_err("held");
    assert_eq!((e.code, e.exit), ("sim_held", 2));
    assert!(m.tree(&a, &agent(), false, None, false).is_ok(), "reads still work while held (Q5)");
    m.hold(&a, &person(), HoldAction::HandBack).expect("hand back");
    let e = m.act(&a, &agent(), &tap("e1"), None).expect_err("read the tree first");
    assert_eq!((e.code, e.exit), ("sim_read_tree", 2));
    let t = m.tree(&a, &agent(), false, None, false).expect("tree");
    let save = save_id(&t);
    m.act(&a, &agent(), &tap(&save), None).expect("acts again after reading");

    // A person's tap holds for 3 s: it can't clear inside the agent's 2 s
    // wait, so the agent fails at once with exit 2.
    m.act(&a, &person(), &tap(&save), None).expect("person tap");
    let t0 = Instant::now();
    let e = m.act(&a, &agent(), &tap(&save), None).expect_err("held by a person's tap");
    assert_eq!((e.code, e.exit), ("sim_held", 2));
    assert!(t0.elapsed() < Duration::from_millis(500), "took {:?}", t0.elapsed());

    // Logs are capped; screenshots land in the workspace at points size.
    let l = m.logs(&a, None, None, None, Some(3)).expect("logs");
    assert_eq!(l.lines.len(), 3);
    assert!(l.truncated);
    let s = m.screenshot(&a, &agent(), None).expect("shot");
    assert!(s.path.starts_with(&tmp.join("a").join(".k2").join("sim").join("shots").display().to_string()), "{}", s.path);
    assert_eq!((s.width, s.height), (400, 800));
    assert!(Path::new(&s.path).is_file());
    let crop = m.screenshot(&a, &agent(), Some(&save)).expect("crop");
    assert_eq!((crop.width, crop.height), (100, 48));

    // Install: only files inside the workspace.
    std::fs::write(tmp.join("a/app.apk"), b"x").expect("apk");
    std::fs::write(tmp.join("outside.apk"), b"x").expect("apk");
    assert_eq!(m.install_app(&a, &agent(), "app.apk").expect("install").outcome.app_id, "app");
    assert_eq!(m.install_app(&a, &agent(), tmp.join("outside.apk").to_str().expect("utf8")).expect_err("outside").code, "forbidden");
    assert_eq!(m.install_app(&a, &agent(), "../outside.apk").expect_err("escape").code, "forbidden");
    assert_eq!(m.launch(&a, &agent(), "com.fake.app; rm -rf /", false).expect_err("bad id").code, "sim_invalid");

    // Stop frees the slot; b boots; erase removes a's data and row.
    assert_eq!(m.stop(&a, &person(), "stopped by owner").expect("stop").state, SimState::Stopped);
    assert_eq!(m.boot(&b, &agent(), &BootReq::default()).expect("boot b").state, SimState::Running);
    let dir_a = crate::sim::device_dir(LaneKind::Android, &crate::sim::workspace_key("p-sim-a"));
    assert!(dir_a.is_dir());
    m.erase(&a, &person()).expect("erase");
    assert!(!dir_a.exists(), "erase reclaims the device data");
    assert!(m.status(&a, false).expect("status").sim.is_none());

    // Idle reaper: no viewer, no verb for longer than the limit.
    limits::save_settings(&limits::SimSettings {
        max_devices: Some(1),
        min_free_ram_bytes: Some(0),
        idle_shutdown_secs: Some(60),
        ..Default::default()
    })
    .expect("limits");
    {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE sim_sessions SET last_used_at = last_used_at - 61 WHERE project_id = 'p-sim-b'", []).expect("age");
    }
    let lookup = |id: &str| (id == "p-sim-b").then(|| b.clone());
    let swept = m.sweep(&lookup);
    assert!(swept.iter().any(|l| l.ends_with("idle stop")), "{swept:?}");
    let row = m.row(&b).expect("row").expect("b row");
    assert_eq!((row.state, row.reason.as_deref()), (SimState::Stopped, Some("idle")));
    let dir_b = crate::sim::device_dir(LaneKind::Android, &crate::sim::workspace_key("p-sim-b"));
    assert!(dir_b.is_dir(), "idle shutdown keeps the data");

    // A device folder whose workspace is gone is erased by the sweep.
    let orphan = crate::sim::device_dir(LaneKind::Android, "0123456789abcdef01234567");
    std::fs::create_dir_all(&orphan).expect("orphan");
    let swept = m.sweep(&lookup);
    assert!(!orphan.exists(), "orphan kept: {swept:?}");
    assert!(dir_b.is_dir(), "a live workspace's folder stays");

    // Adopt after a restart: a recorded device that no longer runs is stopped.
    {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE sim_sessions SET state = 'running' WHERE project_id = 'p-sim-b'", []).expect("fake running");
    }
    let adopted = m.adopt_all();
    assert!(adopted.iter().any(|l| l.ends_with("gone")), "{adopted:?}");
    assert_eq!(m.row(&b).expect("row").expect("b").state, SimState::Stopped);

    // Audit rows for the verbs.
    {
        let db = crate::db::shared();
        let conn = db.lock();
        let actions: Vec<String> = store::recent_audit(&conn, None, 200).expect("audit").into_iter().map(|r| r.action).collect();
        for want in ["boot", "install", "take_over", "hand_back", "stop", "erase", "idle_stop", "adopt_lost"] {
            assert!(actions.iter().any(|a| a == want), "no {want} audit row in {actions:?}");
        }
    }
    crate::sim::clear_lanes();
    let _ = std::fs::remove_dir_all(&tmp);
}

fn boot_one(tag: &str) -> (Manager, Arc<FakeLane>, Ws, PathBuf, i64) {
    let tmp = std::env::temp_dir().join(format!("k2-sim-input-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("w")).expect("ws");
    crate::sim::set_base_for_tests(tmp.join("home"));
    limits::save_settings(&limits::SimSettings { max_devices: Some(2), min_free_ram_bytes: Some(0), ..Default::default() }).expect("limits");
    let fake = Arc::new(FakeLane::new(LaneKind::Android));
    crate::sim::register_lane(fake.clone());
    let id = format!("p-sim-input-{tag}");
    {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute("INSERT OR REPLACE INTO projects (id, name, path) VALUES (?1, ?2, ?3)", rusqlite::params![id, tag, tmp.join("w").display().to_string()]).expect("project");
    }
    let w = ws(&id, &tmp.join("w"));
    let m = Manager::new();
    m.refresh_probes();
    m.boot(&w, &person(), &BootReq { no_tab: true, ..Default::default() }).expect("boot");
    let sim_id = m.row(&w).expect("row").expect("a row").id;
    (m, fake, w, tmp, sim_id)
}

fn touch_at(phase: TouchPhase, x: f32) -> Action {
    Action::Touch { phase, at: Point { x, y: 100.0 } }
}

/// Fix 3 (M-7, M-8): inside a gesture, moves touch no database. The down
/// reads the row and fills the input context; 50 moves read nothing and
/// write nothing; the up reads the row and writes `last_used_at` once. The
/// hold is the person's after the first move and still rolling after the
/// last, so an agent inside it is refused (exit 2) as before.
#[test]
fn a_gestures_moves_skip_the_database_and_keep_the_hold() {
    let _serial = sim_lock();
    let (m, fake, w, tmp, sim) = boot_one("moves");
    m.person_action_sim(sim, &w, "owner", &touch_at(TouchPhase::Down, 0.0)).expect("down");
    let before = m.db_statements();
    for i in 1..=50 {
        m.person_action_sim(sim, &w, "owner", &touch_at(TouchPhase::Move, i as f32)).expect("move");
        if i == 1 {
            assert!(matches!(m.hold_view(sim), HoldView::Person { .. }), "the person holds it after the first move");
        }
    }
    assert_eq!(m.db_statements(), before, "50 moves: no row read, no last_used_at write");
    assert!(matches!(m.hold_view(sim), HoldView::Person { .. }), "still held after the last move");
    let t0 = Instant::now();
    let e = m.act(&w, &agent(), &tap("e1"), None).expect_err("an agent inside the person's hold");
    assert_eq!((e.code, e.exit), ("sim_held", 2));
    assert!(t0.elapsed() < Duration::from_millis(500));
    let before_up = m.db_statements();
    m.person_action_sim(sim, &w, "owner", &touch_at(TouchPhase::Up, 50.0)).expect("up");
    let after_up = m.db_statements();
    assert_eq!(after_up.1 - before_up.1, 1, "one last_used_at write at the up");
    let acts = fake.actions.lock().clone();
    let touches: Vec<&String> = acts.iter().filter(|a| a.starts_with("touch")).collect();
    assert_eq!(touches.len(), 52, "every move reached the device: {acts:?}");
    assert_eq!(touches[0], "touch Down 0,100");
    assert_eq!(touches[51], "touch Up 50,100");
    m.stop(&w, &person(), "test").expect("stop");
    crate::sim::clear_lanes();
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Fix 3: a stop in the middle of a gesture clears the input context; the
/// next move reads the row again and fails with no device; a re-boot
/// starts clean. A lane error clears it too.
#[test]
fn a_stop_mid_gesture_clears_the_input_context() {
    let _serial = sim_lock();
    let (m, fake, w, tmp, sim) = boot_one("stop");
    m.person_action_sim(sim, &w, "owner", &touch_at(TouchPhase::Down, 0.0)).expect("down");
    m.person_action_sim(sim, &w, "owner", &touch_at(TouchPhase::Move, 1.0)).expect("move");
    m.stop(&w, &person(), "stopped by owner").expect("stop");
    let reads = m.db_statements().0;
    let e = m.person_action_sim(sim, &w, "owner", &touch_at(TouchPhase::Move, 2.0)).expect_err("no device");
    assert_eq!(e.code, "sim_no_device");
    assert_eq!(m.db_statements().0, reads + 1, "the move read the row again");
    m.boot(&w, &person(), &BootReq { no_tab: true, ..Default::default() }).expect("re-boot");
    let reads = m.db_statements().0;
    m.person_action_sim(sim, &w, "owner", &touch_at(TouchPhase::Move, 3.0)).expect("a move after the re-boot");
    assert_eq!(m.db_statements().0, reads + 1, "a fresh boot has no context: the first move reads the row");
    // A lane error clears the context: the next move reads the row again.
    fake.running.lock().clear();
    let e = m.person_action_sim(sim, &w, "owner", &touch_at(TouchPhase::Move, 4.0)).expect_err("the device is gone");
    assert_eq!(e.code, "sim_not_running");
    let reads = m.db_statements().0;
    let _ = m.person_action_sim(sim, &w, "owner", &touch_at(TouchPhase::Move, 5.0));
    assert_eq!(m.db_statements().0, reads + 1, "after a lane error the next move reads the row");
    m.stop(&w, &person(), "test").expect("stop");
    crate::sim::clear_lanes();
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Fix 3 keeps the audit as it was: take-over and hand-back write a row
/// each; a person's moves and taps write none.
#[test]
fn take_over_and_hand_back_are_audited_and_moves_are_not() {
    let _serial = sim_lock();
    let (m, _fake, w, tmp, sim) = boot_one("audit");
    let count = || {
        let db = crate::db::shared();
        let conn = db.lock();
        store::recent_audit(&conn, None, 1000).expect("audit").into_iter().filter(|r| r.project_id.as_deref() == Some(w.id.as_str())).count()
    };
    let start = count();
    m.hold(&w, &person(), HoldAction::TakeOver).expect("take over");
    m.person_action_sim(sim, &w, "owner", &touch_at(TouchPhase::Down, 0.0)).expect("down");
    for i in 1..=10 {
        m.person_action_sim(sim, &w, "owner", &touch_at(TouchPhase::Move, i as f32)).expect("move");
    }
    m.person_action_sim(sim, &w, "owner", &touch_at(TouchPhase::Up, 10.0)).expect("up");
    m.person_action_sim(sim, &w, "owner", &Action::Tap { at: Point { x: 1.0, y: 1.0 } }).expect("tap");
    m.hold(&w, &person(), HoldAction::HandBack).expect("hand back");
    assert_eq!(count() - start, 2, "take_over + hand_back, nothing for the input");
    m.stop(&w, &person(), "test").expect("stop");
    crate::sim::clear_lanes();
    let _ = std::fs::remove_dir_all(&tmp);
}
