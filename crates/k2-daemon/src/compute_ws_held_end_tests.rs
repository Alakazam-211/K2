//! A node's ledger snapshot end waits for that attempt's log (the held
//! ends in compute_ws.rs), kept apart so compute_ws.rs stays under the
//! ratchet's line limit.

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
