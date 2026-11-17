//! How a job end a node reports applies here: a ledger snapshot end waits
//! for that attempt's log (the held ends in compute_ws.rs), and a late end
//! keeps the node's own time (CE-5). Kept apart so compute_ws.rs stays
//! under the ratchet's line limit.

use super::tests::job_on;
use super::*;

/// 2026-11-16 (jobs 265ae46f, eae4bb57): a node that (re)attaches
/// sends its ledger snapshot before its shipper sends the logs. An end
/// in that snapshot whose log this server hasn't stored is not
/// published yet (a `done` read then had an empty or cut-off log); the
/// live end the shipper sends after the log applies it, or, if it
/// never comes, the held end applies after `HELD_END_MAX`. An end with
/// nothing more to wait for applies at once.
#[test]
fn a_snapshot_end_waits_for_its_log() {
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    {
        let conn = db.lock();
        store::insert_pending_node(&conn, "n1", "n1", "fp", "pem", &BTreeMap::new(), "owner", "123456", 1).expect("node");
    }
    let running = |id: &str| {
        let j = {
            let conn = db.lock();
            let j = job_on(&conn, id, "n1");
            assert!(store::assign_if_queued(&conn, &j.id, compute::now() - 600).expect("assign"));
            store::job_by_id(&conn, &j.id).expect("row")
        };
        apply_observation("n1", &j.id, j.generation as u32, JobState::Running, None, None, None, None, None, NodeTimes::default());
        let row = store::job_by_id(&db.lock(), &j.id).expect("row");
        assert_eq!(row.state, "running");
        row
    };
    let ended = |j: &store::JobRow, log_seq: u64| frames::LedgerJob {
        job_id: j.id.clone(),
        generation: j.generation as u32,
        attempt: j.attempt as u32,
        plan_digest: String::new(),
        state: JobState::Done,
        reason: None,
        started_at: Some(compute::now() - 500),
        ended_at: Some(compute::now() - 5),
        exit: Some(frames::ExitInfo { code: Some(0), signal: None }),
        log_seq,
        boot_id: "b".into(),
    };
    let state_of = |id: &str| {
        let r = store::job_by_id(&db.lock(), id).expect("row");
        (r.state, r.exit_code)
    };
    // Ids no other test uses (their logs, none stored, are read only).
    let a = running("a5a5a5a50123456789abcdef0e2e1116");
    let b = running("b5b5b5b50123456789abcdef0e2e1116");
    let c = running("c5c5c5c50123456789abcdef0e2e1116");
    assert_eq!(logs::last_seq(&a.id, a.generation), 0, "nothing of its log stored here");
    let mut seen = HashSet::new();
    on_snapshot("n1", JobsSnapshot { jobs: vec![ended(&a, 223), ended(&b, 40), ended(&c, 0)], last: true }, &mut seen);
    assert_eq!(state_of(&a.id), ("running".to_string(), None), "done before its 223 log records are here: held");
    assert_eq!(state_of(&b.id), ("running".to_string(), None));
    assert_eq!(state_of(&c.id), ("done".to_string(), Some(0)), "no log to wait for: applied at once");
    // The shipper's live end (after the log) applies it.
    let live = frames::StateUpdate {
        job_id: a.id.clone(),
        generation: a.generation as u32,
        state: JobState::Done,
        reason: None,
        detail: None,
        at: compute::now() - 5,
        exit: Some(frames::ExitInfo { code: Some(0), signal: None }),
        src: None,
        log_seq: 223,
        outputs: None,
        queue: None,
    };
    on_state("n1", live);
    assert_eq!(state_of(&a.id), ("done".to_string(), Some(0)));
    assert!(held_ends(|m| !m.contains_key(&(a.id.clone(), a.generation as u32))), "the live end replaced the held one");
    // No live end within the limit: the held end applies then, not before.
    release_held_ends(Instant::now() + HELD_END_MAX / 2);
    assert_eq!(state_of(&b.id), ("running".to_string(), None), "still within the limit");
    release_held_ends(Instant::now() + HELD_END_MAX + Duration::from_secs(1));
    assert_eq!(state_of(&b.id), ("done".to_string(), Some(0)), "applied at the limit");
    assert!(held_ends(|m| !m.contains_key(&(b.id.clone(), b.generation as u32))));
}

/// CE-5 acceptance 1: an end the node stamped at T, heard at T + 3 h,
/// keeps T: the duration is the node's, and the job is reported late
/// by ~10800 s. A node whose clock is off keeps this server's time.
#[test]
fn node_times_keep_a_late_end_at_the_nodes_time() {
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    let now = compute::now();
    let mk = |id: &str, node: &str, skew: i64| {
        let conn = db.lock();
        if store::node_by_id(&conn, node).is_none() {
            store::insert_pending_node(&conn, node, node, &format!("fp-{node}"), "pem", &BTreeMap::new(), "owner", "123456", 1).expect("node");
            // A health frame taken at the node's `at`, heard now: the skew.
            let h = k2_core::compute::proto::health::Health { at: now + skew, boot_id: "b".into(), ..Default::default() };
            compute::admin::store_health(&conn, node, now, &h).expect("health");
        }
        let j = job_on(&conn, id, node);
        assert!(store::assign_if_queued(&conn, &j.id, now - 12_000).expect("assign"));
        store::job_by_id(&conn, &j.id).expect("row")
    };
    let job = mk("eeeeeeee0123456789abcdef01234567", "n1", 2);
    let t = now - 10_800;
    apply_observation("n1", &job.id, job.generation as u32, JobState::Running, None, None, None, None, None, NodeTimes { at: Some(t - 600), started_at: None });
    let ex = frames::ExitInfo { code: Some(0), signal: None };
    apply_observation("n1", &job.id, job.generation as u32, JobState::Done, None, None, Some(&ex), None, None, NodeTimes { at: Some(t), started_at: None });
    let row = store::job_by_id(&db.lock(), &job.id).expect("row");
    assert_eq!((row.started_at, row.ended_at), (Some(t - 600), Some(t)), "the node's times: {row:?}");
    assert_eq!(row.ctl.clock.as_deref(), Some("node"));
    let late = row.ctl.reported_late_secs.expect("reported late");
    assert!((10_800..=10_805).contains(&late), "late {late}");
    let env = compute::result::envelope(&row, Some("n1"));
    assert_eq!(env["durationSecs"], 600, "the node's duration, not the sleep: {env}");
    assert_eq!(env["clock"], "node");

    // A node whose clock is 10 min off: this server's time, said so.
    let skewed = mk("ffffffff0123456789abcdef01234567", "n2", 600);
    apply_observation("n2", &skewed.id, skewed.generation as u32, JobState::Running, None, None, None, None, None, NodeTimes { at: Some(t), started_at: None });
    let row = store::job_by_id(&db.lock(), &skewed.id).expect("row");
    assert_eq!(row.ctl.clock.as_deref(), Some("server"));
    assert!(row.started_at.is_some_and(|s| s >= now), "server time: {row:?}");
    assert_eq!(row.ctl.reported_late_secs, None);
}
