//! The scheduler driven by hand: a fake fetch, a scoped DB, both clocks
//! under the test's control.

use super::*;
use crate::egress::test_util::AirgapEnv;
use crate::sources::fetch::Fetched;
use crate::sources::test_engine::Harness;
use k2_core::sources::spec::SourceSpec;
use k2_core::sources::store::NewSource;
use k2_core::sources::{Health, SourceStatus};
use serde_json::json;
use std::sync::{Arc, Mutex};

const T0: i64 = 1_760_000_000;

fn add(conn: &Connection, v: serde_json::Value, lan: bool) -> SourceRow {
    let s: SourceSpec = serde_json::from_value(v).expect("spec");
    let val = s.validate(lan).expect("valid");
    store::insert_source(
        conn,
        &NewSource { v: &val, status: SourceStatus::Active, made_by: "owner", made_by_project: None, approved_by: Some("owner"), now: T0 },
    )
    .expect("insert")
}

fn fleet(n: usize, every: u32) -> serde_json::Value {
    let targets: Vec<_> = (0..n).map(|i| json!({"key": format!("b{i}"), "url": format!("https://b{i}.example.com/boot-status")})).collect();
    json!({"name": "fleet", "label": "Fleet", "fetch": {"targets": targets}, "everySecs": every,
           "extract": {"items": {"itemFields": {"phase": "/phase"}}},
           "rules": [{"field": "phase", "ok": {"eq": "ready"}, "warn": {"ne": "ready"}}]})
}

fn ok_body(v: serde_json::Value) -> Fetched {
    Fetched {
        outcome: FetchOutcome::Ok { status: 200, body: v, not_modified: false, etag: Some("\"e1\"".into()), last_modified: None, elapsed_ms: 1 },
        park_secs: None,
    }
}

fn fail(r: Reason) -> Fetched {
    Fetched { outcome: FetchOutcome::Err { reason: r, http_status: None, retry_after_secs: None }, park_secs: None }
}

/// Run until every source has been polled once (first polls land within
/// min(every, 60) s).
fn warm(h: &mut Harness, conn: &Connection) {
    h.step(conn);
    h.advance(61);
    h.step(conn);
}

#[test]
fn a_down_box_turns_into_a_red_item_after_soft_rechecks() {
    let _a = AirgapEnv::set(false);
    let g = k2_core::db::scoped_for_test();
    let db = g.handle();
    let conn = db.lock();
    let row = add(&conn, fleet(3, 300), false);
    let mut h = Harness::new(
        |p| if p.target_key.as_deref() == Some("b2") { fail(Reason::Timeout) } else { ok_body(json!({"phase": "ready"})) },
        T0,
    );
    warm(&mut h, &conn);
    let (st, _) = store::load_state(&conn, &row.id).unwrap().unwrap();
    assert_eq!(st.items["b2"].soft_count, 1);
    assert_eq!(st.phase, Phase::Live);
    // Soft re-checks come at min(every, 60) = 60 s, not every 300 s.
    for _ in 0..2 {
        h.advance(60);
        h.step(&conn);
    }
    let (st, _) = store::load_state(&conn, &row.id).unwrap().unwrap();
    assert_eq!(st.items["b2"].health, Health::Crit);
    assert_eq!(st.items["b2"].reason, Some(Reason::Timeout));
    assert_eq!(st.phase, Phase::Live, "one down box never stales the source [GV55]");
    let ts = h.transitions();
    assert!(ts.iter().any(|(n, t, gap)| n == "fleet" && t.scope_item.as_deref() == Some("b2") && t.kind == TransitionKind::WentCrit && !gap));
    // Then back to the normal interval (hard non-ok readings return to `every`).
    let next = st.next_at.unwrap() - st.checked_at.unwrap();
    assert!((270..=330).contains(&next), "every × U(0.9, 1.1): {next}");
    let ev = store::events(&conn, &row.id, 50).unwrap();
    assert!(ev.iter().any(|e| e.kind == "went_crit" && e.item_key.as_deref() == Some("b2")));
}

#[test]
fn delays_jitter_soft_and_backoff() {
    let spec: SourceSpec = serde_json::from_value(fleet(1, 120)).unwrap();
    for _ in 0..200 {
        assert!(first_delay(120) <= Duration::from_secs(60));
        assert!(first_delay(30) <= Duration::from_secs(30));
        let d = next_delay(&SourceState::new("x"), &spec).as_secs_f64();
        assert!((108.0..=132.0).contains(&d), "{d}");
    }
    let mut st = SourceState::new("x");
    st.soft_count = 1;
    st.soft_health = Some(k2_core::sources::Health::Warn);
    assert_eq!(next_delay(&st, &spec), Duration::from_secs(60), "a waiting reading re-checks sooner");
    st.soft_health = None;
    st.soft_count = 2;
    assert!(next_delay(&st, &spec) > Duration::from_secs(100), "a streak with nothing waiting keeps the normal pace");
    st.soft_count = 0;
    st.fetch_failures = 3;
    st.backoff_k = 0;
    assert_eq!(next_delay(&st, &spec), Duration::from_secs(120));
    st.backoff_k = 2;
    assert_eq!(next_delay(&st, &spec), Duration::from_secs(480));
    st.backoff_k = 10;
    assert_eq!(next_delay(&st, &spec), Duration::from_secs(900), "capped at max(every, 15 min)");
}

#[test]
fn global_cap_eight_in_flight_and_behind_by() {
    let _a = AirgapEnv::set(false);
    let g = k2_core::db::scoped_for_test();
    let db = g.handle();
    let conn = db.lock();
    add(&conn, fleet(20, 30), false);
    let t0 = Instant::now();
    let mut s = Sched::new(t0, T0);
    let mut post = Post::default();
    assert!(s.tick(&conn, t0, T0, &mut post).len() <= 8, "loads and schedules");
    let t = t0 + Duration::from_secs(61);
    let jobs = s.tick(&conn, t, T0 + 61, &mut post);
    assert_eq!(jobs.len(), MAX_CONCURRENT_FETCHES);
    assert_eq!(s.in_flight(), 8);
    // Nothing more while all eight are out.
    assert!(s.tick(&conn, t + Duration::from_secs(5), T0 + 66, &mut post).is_empty());
    assert!(s.behind_secs() >= 5, "queued fetches show how far behind: {}", s.behind_secs());
    let mut it = jobs.into_iter();
    let j = it.next().unwrap();
    s.complete(&conn, j, ok_body(json!({"phase": "ready"})), t + Duration::from_secs(5), T0 + 66, &mut post);
    assert_eq!(s.tick(&conn, t + Duration::from_secs(5), T0 + 66, &mut post).len(), 1, "a freed slot is refilled");
}

#[test]
fn one_origin_is_held_to_its_budget() {
    let _a = AirgapEnv::set(false);
    let g = k2_core::db::scoped_for_test();
    let db = g.handle();
    let conn = db.lock();
    let targets: Vec<_> = (0..6).map(|i| json!({"key": format!("p{i}"), "url": format!("https://api.example.com/p/{i}")})).collect();
    add(&conn, json!({"name": "same", "label": "Same", "fetch": {"targets": targets}, "everySecs": 60}), false);
    let t0 = Instant::now();
    let mut s = Sched::new(t0, T0);
    let mut post = Post::default();
    assert!(s.tick(&conn, t0, T0, &mut post).len() <= 4);
    let t = t0 + Duration::from_secs(61);
    assert_eq!(s.tick(&conn, t, T0 + 61, &mut post).len(), 4, "burst 4 per origin");
    assert_eq!(s.tick(&conn, t + Duration::from_secs(2), T0 + 63, &mut post).len(), 1, "then one per 2 s");
}

#[test]
fn retry_after_parks_the_origin_and_the_next_cycle_says_rate_limited() {
    let _a = AirgapEnv::set(false);
    let g = k2_core::db::scoped_for_test();
    let db = g.handle();
    let conn = db.lock();
    let row = add(&conn, json!({"name": "gh", "label": "GH", "fetch": {"url": "https://api.github.com/repos/o/r/releases/latest"},
                               "everySecs": 30, "extract": {"fields": {"tag": "/tag_name"}}}), false);
    let calls = Arc::new(Mutex::new(0usize));
    let c2 = calls.clone();
    let mut h = Harness::new(
        move |_| {
            *c2.lock().unwrap() += 1;
            Fetched {
                outcome: FetchOutcome::Err { reason: Reason::RateLimited, http_status: Some(429), retry_after_secs: Some(600) },
                park_secs: Some(600),
            }
        },
        T0,
    );
    warm(&mut h, &conn);
    assert_eq!(*calls.lock().unwrap(), 1);
    let parks = h.sched.budget.parks(h.mono);
    assert_eq!(parks.len(), 1);
    assert_eq!(parks[0].0, "https://api.github.com:443");
    // Re-checks while parked fetch nothing.
    for _ in 0..5 {
        h.advance(60);
        h.step(&conn);
    }
    assert_eq!(*calls.lock().unwrap(), 1, "no request while the origin is parked");
    let (st, _) = store::load_state(&conn, &row.id).unwrap().unwrap();
    assert_eq!(st.reason, Some(Reason::RateLimited));
    h.advance(600);
    h.step(&conn);
    assert_eq!(*calls.lock().unwrap(), 2, "polls again once the park lifts");
}

#[test]
fn etag_rides_the_next_request_and_a_304_keeps_the_data() {
    let _a = AirgapEnv::set(false);
    let g = k2_core::db::scoped_for_test();
    let db = g.handle();
    let conn = db.lock();
    let row = add(&conn, json!({"name": "st", "label": "St", "fetch": {"url": "https://status.example.com/s.json"},
                               "everySecs": 30, "extract": {"fields": {"v": "/v"}}}), false);
    let seen: Arc<Mutex<Vec<Vec<(String, String)>>>> = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    let mut h = Harness::new(
        move |p| {
            s2.lock().unwrap().push(p.headers.clone());
            if p.headers.iter().any(|(k, _)| k == "If-None-Match") {
                Fetched {
                    outcome: FetchOutcome::Ok { status: 304, body: serde_json::Value::Null, not_modified: true, etag: None, last_modified: None, elapsed_ms: 1 },
                    park_secs: None,
                }
            } else {
                ok_body(json!({"v": 7}))
            }
        },
        T0,
    );
    warm(&mut h, &conn);
    h.advance(40);
    h.step(&conn);
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert!(seen[1].contains(&("If-None-Match".to_string(), "\"e1\"".to_string())));
    let (st, p) = store::load_state(&conn, &row.id).unwrap().unwrap();
    assert_eq!(p.unwrap().fields["v"], json!(7));
    assert_eq!(st.updated_at, st.checked_at, "a 304 is a success");
}

#[test]
fn a_host_sleep_gaps_sources_and_the_next_poll_is_flagged_in_gap() {
    let _a = AirgapEnv::set(false);
    let g = k2_core::db::scoped_for_test();
    let db = g.handle();
    let conn = db.lock();
    let row = add(&conn, fleet(1, 60), false);
    let up = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let u2 = up.clone();
    let mut h = Harness::new(
        move |_| if u2.load(std::sync::atomic::Ordering::SeqCst) { ok_body(json!({"phase": "ready"})) } else { fail(Reason::Timeout) },
        T0,
    );
    warm(&mut h, &conn);
    for _ in 0..3 {
        h.advance(60);
        h.step(&conn);
    }
    let (st, _) = store::load_state(&conn, &row.id).unwrap().unwrap();
    assert_eq!(st.items["b0"].health, Health::Crit);
    h.posts.clear();
    // 6 h on the wall clock, no time on the monotonic one: the lid was shut.
    h.sleep(6 * 3600);
    h.step(&conn);
    let (st, _) = store::load_state(&conn, &row.id).unwrap().unwrap();
    assert!(st.in_gap(), "{:?}", st.phase);
    assert_eq!(st.gap_kind, Some(GapKind::HostSlept));
    assert_eq!(h.sched.last_gap.map(|g| g.0), Some(GapKind::HostSlept));
    let ev = store::events(&conn, &row.id, 10).unwrap();
    assert!(ev.iter().any(|e| e.kind == "gap" && e.reason.as_deref() == Some("host_slept")));
    // The box came back while the Mac slept: the first poll after the gap
    // recovers it, and that transition is flagged so triggers suppress it.
    up.store(true, std::sync::atomic::Ordering::SeqCst);
    h.advance(61);
    h.step(&conn);
    let ts: Vec<_> = h.transitions().into_iter().filter(|(_, t, _)| t.kind == TransitionKind::Recovered).collect();
    assert!(!ts.is_empty(), "recovered after the gap");
    assert!(ts.iter().all(|(_, _, in_gap)| *in_gap), "transitions right after a gap are flagged");
    let (st, _) = store::load_state(&conn, &row.id).unwrap().unwrap();
    assert!(!st.in_gap());
}

#[test]
fn a_restart_marks_daemon_restarted_and_a_backwards_clock_step_doesnt_stall() {
    let _a = AirgapEnv::set(false);
    let g = k2_core::db::scoped_for_test();
    let db = g.handle();
    let conn = db.lock();
    let row = add(&conn, fleet(1, 60), false);
    let mut h = Harness::new(|_| ok_body(json!({"phase": "ready"})), T0);
    warm(&mut h, &conn);
    // A new scheduler (the daemon restarted 5 minutes later).
    let mut s = Sched::new(Instant::now(), T0 + 400);
    let mut post = Post::default();
    s.boot(&conn, Instant::now(), T0 + 400, &mut post);
    let (st, _) = store::load_state(&conn, &row.id).unwrap().unwrap();
    assert_eq!((st.phase, st.gap_kind), (Phase::Paused, Some(GapKind::DaemonRestarted)));
    assert_eq!(s.last_gap.map(|g| g.0), Some(GapKind::DaemonRestarted));
    assert!(!post.notify.is_empty());
    // The NTP clock steps back an hour; the monotonic heap still fires.
    let mut h2 = Harness { sched: s, fetch: Box::new(|_| ok_body(json!({"phase": "ready"}))), mono: Instant::now(), wall: T0 + 400, posts: vec![] };
    h2.wall -= 3600;
    h2.advance(61);
    assert_eq!(h2.step(&conn), 1, "polled despite the backwards wall step");
    let (st, _) = store::load_state(&conn, &row.id).unwrap().unwrap();
    assert_eq!(st.phase, Phase::Live);
    assert!(st.gap_kind.is_none());
}

#[test]
fn air_gap_pauses_name_sources_without_fetching() {
    let _a = AirgapEnv::set(true);
    let g = k2_core::db::scoped_for_test();
    let db = g.handle();
    let conn = db.lock();
    let row = add(&conn, fleet(2, 30), false);
    let calls = Arc::new(Mutex::new(0usize));
    let c2 = calls.clone();
    let mut h = Harness::new(move |_| {
        *c2.lock().unwrap() += 1;
        ok_body(json!({}))
    }, T0);
    warm(&mut h, &conn);
    assert_eq!(*calls.lock().unwrap(), 0);
    let (st, _) = store::load_state(&conn, &row.id).unwrap().unwrap();
    assert_eq!((st.phase, st.reason), (Phase::Paused, Some(Reason::Airgap)));
}

#[test]
fn draining_starts_no_fetch() {
    let _a = AirgapEnv::set(false);
    let g = k2_core::db::scoped_for_test();
    let db = g.handle();
    let conn = db.lock();
    add(&conn, fleet(2, 30), false);
    let mut s = Sched::new(Instant::now(), T0);
    s.draining = true;
    let mut post = Post::default();
    assert!(s.tick(&conn, Instant::now() + Duration::from_secs(61), T0 + 61, &mut post).is_empty());
}

#[test]
fn an_expired_mute_is_cleared_and_reported() {
    let _a = AirgapEnv::set(false);
    let g = k2_core::db::scoped_for_test();
    let db = g.handle();
    let conn = db.lock();
    let row = add(&conn, fleet(1, 60), false);
    store::set_mute(&conn, &row.id, Some(T0 + 100), Some("owner"), Some("roll"), T0).unwrap();
    let mut h = Harness::new(|_| ok_body(json!({"phase": "ready"})), T0);
    h.step(&conn);
    assert!(h.posts.iter().all(|p| p.unmuted.is_empty()));
    h.advance(101);
    h.step(&conn);
    assert!(h.posts.iter().any(|p| p.unmuted.iter().any(|r| r.id == row.id)));
    assert_eq!(store::get_by_id(&conn, &row.id).unwrap().unwrap().mute_until, None);
}

/// The Fleet Ops shape end to end through the scheduler and the store: one
/// K2 Web summary with boxes, versions, the fleet target and the block's own
/// timestamp. Behind target warns at once (failAfter 3 never delays it), a
/// minor behind goes crit after 3 checks in a row (soft re-checks every
/// 60 s), and an old `ts` that is still served goes crit with no fetch
/// failing.
#[test]
fn fleet_ops_fixture_through_the_scheduler() {
    let _a = AirgapEnv::set(false);
    let g = k2_core::db::scoped_for_test();
    let db = g.handle();
    let conn = db.lock();
    let row = add(
        &conn,
        json!({
            "name": "k2-ops-boxes", "label": "K2 boxes", "fetch": {"url": "https://k2.dev/api/ops/fleet?part=boxes"},
            "everySecs": 300, "failAfter": 3,
            "extract": {
                "fields": {"target_version": "/target_version", "ts": "/ts"},
                "items": {"pointer": "/boxes", "itemKey": "/name", "itemFields": {"version": "/version"}},
                "aggregates": {"behind": {"op": "countWhere", "field": "version",
                                          "cond": {"versionBehind": {"field": "target_version"}}}}
            },
            "rules": [
                {"field": "ts", "warn": {"ageGt": 300}, "crit": {"ageGt": 900}, "ok": {"exists": true}},
                {"field": "version", "ok": {"versionAtLeast": {"field": "target_version"}},
                 "warn": {"versionBehind": {"field": "target_version"}},
                 "crit": {"versionBehind": {"field": "target_version", "minor": 1}}, "critAfter": 3}
            ]
        }),
        false,
    );
    let ts = Arc::new(Mutex::new(T0 + 100_000));
    let ts2 = ts.clone();
    let mut h = Harness::new(
        move |_| {
            let t = *ts2.lock().unwrap();
            ok_body(json!({"ts": t, "target_version": "0.46.0", "boxes": [
                {"name": "akzm", "version": "0.46.0"}, {"name": "ohana", "version": "0.44.4"}, {"name": "luzz", "version": "0.45.9"}]}))
        },
        T0,
    );
    warm(&mut h, &conn);
    let (st, proj) = store::load_state(&conn, &row.id).unwrap().unwrap();
    assert_eq!(st.items["luzz"].health, Health::Warn, "behind target warns on the first check");
    assert_eq!((st.items["ohana"].health, st.items["ohana"].soft_health), (Health::Warn, Some(Health::Crit)), "soft crit 1/3");
    assert_eq!(st.items["akzm"].health, Health::Ok);
    assert_eq!(proj.as_ref().unwrap().fields["behind"], json!(2));
    // The card view says so too.
    let row_now = store::get_by_id(&conn, &row.id).unwrap().unwrap();
    let v = k2_core::sources::view::build(&row_now, &st, proj.as_ref(), k2_core::sources::Viewer::Person, h.wall);
    let ohana = v.items.iter().find(|i| i.key == "ohana").unwrap();
    assert_eq!((ohana.soft.count, ohana.soft.of, ohana.soft.level), (1, 3, Some(Health::Crit)));
    for _ in 0..2 {
        h.advance(60);
        h.step(&conn);
    }
    let (st, _) = store::load_state(&conn, &row.id).unwrap().unwrap();
    assert_eq!(st.items["ohana"].health, Health::Crit, "crit after 3 checks, re-checked every 60 s");
    let ev = store::events(&conn, &row.id, 50).unwrap();
    assert!(ev.iter().any(|e| e.kind == "went_crit" && e.item_key.as_deref() == Some("ohana")));
    assert!(ev.iter().any(|e| e.kind == "went_warn" && e.item_key.as_deref() == Some("luzz")));
    // The block's ts goes old while still served.
    *ts.lock().unwrap() = T0 - 10_000;
    h.advance(400);
    h.step(&conn);
    let (st, _) = store::load_state(&conn, &row.id).unwrap().unwrap();
    assert_eq!(st.self_health, Some(Health::Crit), "stale data reads crit");
    assert_eq!(st.phase, Phase::Live, "the fetch itself is fine");
    assert_eq!(st.fetch_failures, 0);
}
