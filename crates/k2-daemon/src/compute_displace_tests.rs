//! Displacement's rules (0.46.18): the pure choice, then the whole path
//! on a fake node session (the sweep, the node's stop, the same job queued
//! again, the CI job taking the slot).

use super::*;
use k2_core::compute::proto::frames::{self, Frame, JobLimits, Plan, StateUpdate};
use k2_core::compute::proto::tiers::{FEATURE_TIERS, FEATURE_TIERS2};
use std::collections::BTreeMap;

const GB: u64 = 1 << 30;

// ── the rules ────────────────────────────────────────────────────────

#[test]
fn a_soak_is_batch_or_over_two_hours_and_unknown_is_not_one() {
    assert!(is_soak(JobTier::Batch, None, None), "Batch is a soak");
    assert!(is_soak(JobTier::Normal, Some(SOAK_SECS + 1), None), "expected over 2 h");
    assert!(is_soak(JobTier::Normal, None, Some(SOAK_SECS + 1)), "--timeout over 2 h");
    assert!(!is_soak(JobTier::Normal, Some(SOAK_SECS), Some(SOAK_SECS)), "exactly 2 h is not over");
    assert!(!is_soak(JobTier::Normal, None, None), "an unknown run time is not a soak");
}

fn row(id: &str, tier: &str) -> JobRow {
    JobRow { id: id.into(), tier: tier.into(), state: "running".into(), ..Default::default() }
}

#[test]
fn only_our_running_standard_non_soak_jobs_are_eligible() {
    let ci = "ci-job";
    assert!(eligible(&row("s", "normal"), ci));
    for t in ["ci", "priority", "batch"] {
        assert!(!eligible(&row("x", t), ci), "{t} is never displaced");
    }
    assert!(!eligible(&JobRow { est_secs: Some(3 * 3600), ..row("long", "normal") }, ci), "expected 3 h: a soak");
    let timed = JobRow { ctl: store::JobCtl { timeout_secs: Some(3 * 3600), ..Default::default() }, ..row("t", "normal") };
    assert!(!eligible(&timed, ci), "--timeout 3h: a soak");
    assert!(!eligible(&JobRow { state: "finishing".into(), ..row("f", "normal") }, ci), "finishing: left alone");
    assert!(!eligible(&JobRow { node_queued: true, state: "assigned".into(), ..row("q", "normal") }, ci), "in a node's queue: not running");
    assert!(!eligible(&JobRow { reason: Some("cancel_requested".into()), ..row("c", "normal") }, ci), "being cancelled");
    assert!(!eligible(&JobRow { reason: Some(DISPLACE_REQUESTED.into()), ..row("d", "normal") }, ci), "already being ended");
    let once = JobRow {
        ctl: store::JobCtl { displaced: vec![Displaced { by: ci.into(), node_id: "n".into(), generation: 1, pinned: true, ..Default::default() }], ..Default::default() },
        ..row("p", "normal")
    };
    assert!(!eligible(&once, ci), "at most once per CI job");
    assert!(eligible(&once, "another-ci"), "another CI job may");
}

fn v(id: &str, ran: i64, warm: bool) -> Victim {
    Victim { id: id.into(), ran_secs: ran, warm, cpu_millis: 1000, mem_bytes: GB }
}

#[test]
fn least_run_goes_first_and_a_warm_cache_goes_last_among_equals() {
    let mut list = vec![v("old", 3600, false), v("warm", 70, true), v("cold", 100, false), v("young", 5, false)];
    order(&mut list);
    let ids: Vec<&str> = list.iter().map(|x| x.id.as_str()).collect();
    assert_eq!(ids, ["young", "cold", "warm", "old"], "warm and cold ran the same minute: cold first");
}

fn solo_offer(parallel: u32, running: u32) -> Offer {
    Offer {
        share: None,
        revision: 1,
        control: frames::Control::Active,
        control_by: None,
        control_at: None,
        availability: frames::Availability { ok: true, reasons: vec![] },
        caps: frames::Caps { cpu_millis: 16000, mem_bytes: 64 * GB, disk_budget_bytes: 100 * GB, hard: true, gpus: 0 },
        free: frames::Free { cpu_millis: 16000 - u64::from(running) * 1000, mem_bytes: 64 * GB - u64::from(running) * GB, disk_free_bytes: 500 * GB, gpus: 0 },
        slots: vec![],
        running,
        parallel_max: parallel,
        refusal: None,
        labels: [("os".to_string(), "linux".to_string())].into(),
        tools: BTreeMap::new(),
        protocol: 6,
        boot_id: "b".into(),
        ledger_id: "l".into(),
        node_version: "0.46.18".into(),
        features: vec![FEATURE_TIERS.to_string(), FEATURE_TIERS2.to_string()],
        host: None,
        pkg: None,
        sim: None,
    }
}

#[test]
fn a_full_node_needs_one_slot_and_the_youngest_covers_it() {
    let need = need_on(&solo_offer(2, 2), &Pending::default(), Some(1000), Some(GB), false);
    assert_eq!(need, Need { slots: 1, cpu_millis: 0, mem_bytes: 0, idle: false });
    let list = vec![v("young", 5, false), v("old", 600, false)];
    assert_eq!(pick(&need, &list, 2), Some(vec!["young".to_string()]));
    assert_eq!(pick(&need, &[], 2), None, "nothing eligible: nothing picked");
    assert_eq!(pick(&need_on(&solo_offer(2, 1), &Pending::default(), Some(1000), Some(GB), false), &list, 1), None, "a free slot: nothing to end");
    // An exclusive CI job needs the node idle: every running job must go.
    let idle = need_on(&solo_offer(4, 2), &Pending::default(), None, None, true);
    assert!(idle.idle);
    assert_eq!(pick(&idle, &list, 2), Some(vec!["young".to_string(), "old".to_string()]));
    assert_eq!(pick(&idle, &list[..1], 2), None, "one running job isn't ours to end");
}

fn look(name: &str, starts_now: bool, soon: Option<u64>, victims: Vec<Victim>) -> Look {
    let need = if starts_now { Need::default() } else { Need { slots: 1, ..Default::default() } };
    Look { node_id: format!("id-{name}"), name: name.into(), starts_now, soon, need, victims, running: 2 }
}

#[test]
fn go_elsewhere_first_then_the_first_node_with_a_job_to_end() {
    let full = || look("node-a", false, None, vec![v("young", 5, false)]);
    assert_eq!(decide(&[full(), look("node-b", true, None, vec![])]), Step::Placing, "another node starts it now");
    match decide(&[full(), look("node-b", false, Some(240), vec![])]) {
        Step::Wait(w) => assert!(w.contains("node-b should free a slot within"), "{w}"),
        other => panic!("a node frees within 5 min: wait, got {other:?}"),
    }
    assert_eq!(
        decide(&[full(), look("node-b", false, Some(ELSEWHERE_SECS + 60), vec![])]),
        Step::Displace { node_id: "id-node-a".into(), victims: vec!["young".into()] },
        "nothing frees within 5 min: displace"
    );
    assert_eq!(
        decide(&[look("node-b", false, None, vec![]), full()]),
        Step::Displace { node_id: "id-node-a".into(), victims: vec!["young".into()] },
        "the first node with something to end"
    );
    assert_eq!(decide(&[look("node-b", false, None, vec![])]), Step::Wait("waiting: no displaceable job on node-b".into()));
}

#[test]
fn a_displaced_auto_job_avoids_the_node_for_thirty_minutes_and_a_pinned_one_never() {
    let note = |pinned: bool| Displaced { by: "ci".into(), node_id: "node-a".into(), at: 1_000, generation: 1, pinned, ..Default::default() };
    let auto = store::JobCtl { displaced: vec![note(false)], ..Default::default() };
    assert_eq!(avoiding(&auto, 1_000 + AVOID_SECS - 1), vec!["node-a".to_string()]);
    assert!(avoiding(&auto, 1_000 + AVOID_SECS).is_empty(), "after 30 min it may go back");
    let pinned = store::JobCtl { displaced: vec![note(true)], ..Default::default() };
    assert!(avoiding(&pinned, 1_001).is_empty(), "a pinned job is next on its node");
}

#[test]
fn a_stop_or_lost_run_is_queued_again_and_a_finished_one_keeps_its_result() {
    for st in [JobState::Cancelled, JobState::Interrupted, JobState::Unknown] {
        assert!(owes_requeue(Some(DISPLACE_REQUESTED), st), "{st:?}");
    }
    for st in [JobState::Done, JobState::Failed, JobState::Timeout, JobState::Running] {
        assert!(!owes_requeue(Some(DISPLACE_REQUESTED), st), "{st:?} keeps its result");
    }
    assert!(!owes_requeue(Some("cancel_requested"), JobState::Cancelled), "a person's cancel ends it");
    assert!(!owes_requeue(None, JobState::Cancelled));
}

// ── the whole path on fake node sessions ─────────────────────────────

fn node(conn: &rusqlite::Connection, id: &str, grants: &[&str]) {
    store::insert_pending_node(conn, id, id, &format!("fp-{id}"), "pem", &BTreeMap::new(), "owner", "123456", 1).expect("node");
    store::confirm_node(conn, id, "owner", 2).expect("confirm");
    for ws in grants {
        let limits = store::GrantLimits { max_parallel: Some(4), max_job_secs: Some(6 * 3600), allow_ci: Some(true), ..Default::default() };
        store::set_grant(conn, id, ws, &limits, "owner", 3).expect("grant");
    }
}

#[allow(clippy::too_many_arguments)]
fn job(conn: &rusqlite::Connection, id: &str, node: &str, ws: &str, tier: &str, auto: bool, est: Option<i64>, timeout: Option<i64>, created_at: i64) -> JobRow {
    let plan = Plan {
        class: None,
        session_id: None,
        est_secs: None,
        est_p90_secs: None,
        ws_max_running: None,
        job_id: id.into(),
        node_id: node.into(),
        workspace_id: ws.into(),
        workspace_label: ws.into(),
        requested_by: format!("agent:{ws}"),
        argv: vec!["true".into()],
        env: BTreeMap::new(),
        cwd: None,
        limits: JobLimits { max_secs: 3600, cpu_millis: Some(1000), mem_bytes: Some(GB), disk_bytes: 1, log_cap_bytes: 1 },
        src: None,
        exclusive: false,
        created_at,
        needs: Vec::new(),
        artifacts: Vec::new(),
        stall_secs: None,
        secret_env: Vec::new(),
        tier: None,
        cache: None,
        src_path: None,
        submitter: None,
        submitter_max_running: None,
        packages: Vec::new(),
        git_access: Vec::new(),
        receipt_v: None,
        gpu: None,
        gpu_mem: None,
        cache_commit: None,
        cache_ref: None,
        setup: Vec::new(),
        cpus_burst: None,
        provenance: None,
        network: None,
        userns: false,
        no_addr_filter: false,
        with: Vec::new(),
    };
    let ctl = store::JobCtl { any: auto.then(Default::default), timeout_secs: timeout, ..Default::default() };
    store::insert_job(conn, &store::NewJob { plan: &plan, client_job_id: id, session_id: None, request_sha256: "h", detach: false, retry_interrupted: false, ctl }).expect("insert job");
    conn.execute(
        "UPDATE compute_jobs SET tier = ?2, est_secs = ?3, submitter = ?4 WHERE id = ?1",
        rusqlite::params![id, tier, est, format!("{ws}/builder")],
    )
    .expect("tier");
    store::job_by_id(conn, id).expect("row")
}

fn running(conn: &rusqlite::Connection, id: &str, since: i64) {
    let n = conn
        .execute("UPDATE compute_jobs SET state = 'running', assigned_at = ?2, started_at = ?2 WHERE id = ?1", rusqlite::params![id, since])
        .expect("running");
    assert_eq!(n, 1);
}

fn get(id: &str) -> JobRow {
    k2_core::db::read(|conn| store::job_by_id(conn, id)).expect("row")
}

fn cancels(frames: &[Frame]) -> Vec<(String, String)> {
    frames
        .iter()
        .filter_map(|f| match f {
            Frame::Cancel(c) => Some((c.job_id.clone(), c.reason.clone())),
            _ => None,
        })
        .collect()
}

fn assigns(frames: &[Frame]) -> Vec<String> {
    frames
        .iter()
        .filter_map(|f| match f {
            Frame::Assign(a) => Some(a.fence.job_id.clone()),
            _ => None,
        })
        .collect()
}

fn stopped(node_id: &str, j: &JobRow) {
    compute_ws::test_frame(
        node_id,
        Frame::State(StateUpdate {
            job_id: j.id.clone(),
            generation: j.generation as u32,
            state: JobState::Cancelled,
            reason: Some(FRAME_REASON.into()),
            detail: None,
            at: compute::now(),
            exit: None,
            src: None,
            log_seq: 0,
            outputs: None,
            queue: None,
        }),
    );
}

/// The core promise: a CI job pinned to a full node starts within a
/// minute by ending the youngest Standard job there; that job comes back
/// as the same job (same id, a new generation, no retry used), is next on
/// its node, and is never displaced twice by the same CI job.
#[test]
fn a_ci_job_takes_a_full_node_from_the_youngest_standard_job() {
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    let now = compute::now();
    let n = "dsp-full-a";
    let (old, young, ci) = {
        let conn = db.lock();
        node(&conn, n, &["ws-a", "ws-b", "ws-ci"]);
        let old = job(&conn, "d15a0000000000000000000000000001", n, "ws-a", "normal", false, None, None, now - 700);
        let young = job(&conn, "d15a0000000000000000000000000002", n, "ws-b", "normal", false, None, None, now - 100);
        let ci = job(&conn, "d15a0000000000000000000000000003", n, "ws-ci", "ci", false, None, None, now - 30);
        running(&conn, &old.id, now - 600);
        running(&conn, &young.id, now - 60);
        (old, young, ci)
    };
    let mut c = compute_ws::test_attach(n, solo_offer(2, 2));
    c.frames();

    sweep(now);
    let sent = cancels(&c.frames());
    assert_eq!(sent, vec![(young.id.clone(), FRAME_REASON.to_string())], "the youngest Standard job is told to stop");
    assert_eq!(get(&young.id).reason.as_deref(), Some(DISPLACE_REQUESTED));
    assert_eq!(get(&old.id).reason, None, "the older job keeps running");
    assert!(ci_words(&ci.id).is_some_and(|w| w.starts_with("displacing")), "{:?}", ci_words(&ci.id));

    // While the node ends it, nothing more is displaced for this CI job.
    sweep(now + 1);
    assert!(cancels(&c.frames()).is_empty(), "one slot asked for, one job ended");
    assert_eq!(get(&old.id).reason, None);

    // The node stops it: the same job is queued again, never a result.
    let before = get(&young.id);
    stopped(n, &before);
    let back = get(&young.id);
    assert_eq!(back.state, "queued", "queued again, not cancelled");
    assert_eq!(back.generation, before.generation + 1, "a new generation");
    assert_eq!(back.attempt, before.attempt, "the attempt doesn't count");
    assert_eq!(back.ctl.retries_used, 0, "no retry used");
    assert!(back.ctl.failed_attempts.is_empty(), "not a failure");
    assert_eq!(back.node_id, n, "a pinned job stays on its node");
    assert!(next_on_node(&back), "and is next there");
    let words = queued_words(&back, now + 2).expect("its words");
    assert!(words.starts_with(&format!("displaced by {}", message::short(&ci.id))) && words.contains("(ws-ci/builder)") && words.ends_with(&format!("; next on {n}")), "{words}");
    // What `wait --follow` reads: the ended generation's record says it
    // was displaced (a move, not an infrastructure failure).
    let rec = back.ctl.attempts.iter().find(|r| r.generation == before.generation).expect("a record of the ended run");
    assert_eq!(rec.next, next::DISPLACED);
    let names = HashMap::from([(n.to_string(), n.to_string())]);
    let listed = k2_core::compute::attempts::list_json(&back, &names);
    let moved = listed[0].get("moved").and_then(|m| m.as_str()).expect("moved words");
    assert_eq!(moved, format!("displaced by {} (ws-ci/builder), re-queued as the same job", message::short(&ci.id)));

    // The node's next offer shows the free slot: the CI job takes it, not
    // the displaced job (which is next).
    compute_ws::test_frame(n, Frame::Offer(solo_offer(2, 1)));
    let a = assigns(&c.frames());
    assert_eq!(a.first(), Some(&ci.id), "the CI job starts in the freed slot: {a:?}");
    assert!(!a.contains(&young.id), "the displaced job waits for the next slot");

    // At most once per CI job: running again, it is never this CI job's
    // victim a second time.
    running(&db.lock(), &young.id, now + 5);
    assert!(!eligible(&get(&young.id), &ci.id));
    compute_ws::test_detach(n);
}

/// Soaks (Batch, expected over 2 h, `--timeout` over 2 h) are never
/// displaced, a Priority job displaces nothing, and a CI job with nothing
/// to end says so.
#[test]
fn soaks_are_never_displaced_and_only_ci_displaces() {
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    let now = compute::now();
    let n = "dsp-soaks-b";
    let (soaks, pri, ci) = {
        let conn = db.lock();
        node(&conn, n, &["ws-a", "ws-ci"]);
        let batch = job(&conn, "d15b0000000000000000000000000001", n, "ws-a", "batch", false, None, None, now - 100);
        let long = job(&conn, "d15b0000000000000000000000000002", n, "ws-a", "normal", false, Some(3 * 3600), None, now - 100);
        let timed = job(&conn, "d15b0000000000000000000000000003", n, "ws-a", "normal", false, None, Some(3 * 3600), now - 100);
        for j in [&batch, &long, &timed] {
            running(&conn, &j.id, now - 30);
        }
        let pri = job(&conn, "d15b0000000000000000000000000004", n, "ws-a", "priority", false, None, None, now - 60);
        let ci = job(&conn, "d15b0000000000000000000000000005", n, "ws-ci", "ci", false, None, None, now - 60);
        (vec![batch, long, timed], pri, ci)
    };
    let mut c = compute_ws::test_attach(n, solo_offer(3, 3));
    c.frames();
    sweep(now);
    assert!(cancels(&c.frames()).is_empty(), "no soak is ended");
    for j in &soaks {
        assert_eq!(get(&j.id).reason, None, "{} keeps running", j.id);
    }
    assert_eq!(ci_words(&ci.id).as_deref(), Some(format!("waiting: no displaceable job on {n}").as_str()));
    assert_eq!(ci_words(&pri.id), None, "Priority never displaces");

    // A Standard job there now: the CI job ends it; the Priority job still
    // displaces nothing.
    let std = {
        let conn = db.lock();
        let j = job(&conn, "d15b0000000000000000000000000006", n, "ws-a", "normal", false, None, None, now - 50);
        running(&conn, &j.id, now - 20);
        j
    };
    // The 3 soaks plus the new job: the CI job lacks one slot of 4.
    compute_ws::test_frame(n, Frame::Offer(solo_offer(4, 4)));
    c.frames();
    sweep(now + 1);
    assert_eq!(cancels(&c.frames()), vec![(std.id.clone(), FRAME_REASON.to_string())]);
    compute_ws::test_detach(n);
}

/// A pinned CI job whose node should free a slot within 5 minutes waits
/// for it: nothing is displaced.
#[test]
fn a_node_that_frees_soon_is_waited_for() {
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    let now = compute::now();
    let n = "dsp-soon-c";
    let (std, ci) = {
        let conn = db.lock();
        node(&conn, n, &["ws-a", "ws-ci"]);
        let std = job(&conn, "d15c0000000000000000000000000001", n, "ws-a", "normal", false, Some(240), None, now - 100);
        running(&conn, &std.id, now - 60);
        let ci = job(&conn, "d15c0000000000000000000000000002", n, "ws-ci", "ci", false, None, None, now - 60);
        (std, ci)
    };
    let mut c = compute_ws::test_attach(n, solo_offer(1, 1));
    c.frames();
    sweep(now);
    assert!(cancels(&c.frames()).is_empty(), "it ends in about 3 min: wait");
    assert_eq!(get(&std.id).reason, None);
    assert!(ci_words(&ci.id).is_some_and(|w| w.contains("should free a slot within")), "{:?}", ci_words(&ci.id));
    compute_ws::test_detach(n);
}

/// A soak (Batch, or over 2 h) is never auto-placed on a node labelled
/// `release` for this server; Standard work still goes there.
#[test]
fn a_soak_is_never_placed_on_a_release_node() {
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    let now = compute::now();
    let (rel, other) = ("dsp-rel-e1", "dsp-rel-e2");
    let jobs = {
        let conn = db.lock();
        node(&conn, rel, &["ws-a"]);
        node(&conn, other, &["ws-a"]);
        k2_core::compute::server_labels::change(&conn, rel, &[RELEASE_LABEL.to_string()], &[], "owner", 4).expect("label");
        [
            ("batch", job(&conn, "d15e0000000000000000000000000001", other, "ws-a", "batch", true, None, None, now)),
            ("long", job(&conn, "d15e0000000000000000000000000002", other, "ws-a", "normal", true, Some(3 * 3600), None, now)),
            ("timeout", job(&conn, "d15e0000000000000000000000000003", other, "ws-a", "normal", true, None, Some(3 * 3600), now)),
            ("standard", job(&conn, "d15e0000000000000000000000000004", other, "ws-a", "normal", true, Some(600), Some(3600), now)),
        ]
    };
    let mut c1 = compute_ws::test_attach(rel, solo_offer(2, 0));
    let mut c2 = compute_ws::test_attach(other, solo_offer(2, 0));
    c1.frames();
    c2.frames();
    for (what, j) in &jobs {
        let w = Want::of_job(j, &j.plan().expect("plan"));
        let report = compute_place::candidates_report(&w, &[], now);
        let names: Vec<&str> = report.cands.iter().map(|c| c.node.id.as_str()).collect();
        assert!(names.contains(&other), "{what}: {names:?}");
        if *what == "standard" {
            assert!(names.contains(&rel), "Standard work still goes on the release node: {names:?}");
            assert!(!report.skipped.contains_key(compute_place::skip::RELEASE_NODE));
        } else {
            assert!(!names.contains(&rel), "{what} is a soak: never on {rel}: {names:?}");
            assert_eq!(report.skipped.get(compute_place::skip::RELEASE_NODE), Some(&1), "{what}");
        }
    }
    compute_ws::test_detach(rel);
    compute_ws::test_detach(other);
}

/// Go elsewhere first (auto): another node starts the CI job now, so
/// nothing is displaced. Then, with that node full too, the displaced
/// auto job runs again elsewhere, uncounted, and avoids the node.
#[test]
fn an_auto_ci_job_goes_elsewhere_first_and_a_displaced_auto_job_moves_on() {
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    let now = compute::now();
    let (n1, n2) = ("dsp-auto-d1", "dsp-auto-d2");
    let (victim, ci) = {
        let conn = db.lock();
        node(&conn, n1, &["ws-a", "ws-ci"]);
        node(&conn, n2, &["ws-a", "ws-ci"]);
        let victim = job(&conn, "d15d0000000000000000000000000001", n1, "ws-a", "normal", true, None, None, now - 100);
        running(&conn, &victim.id, now - 60);
        let ci = job(&conn, "d15d0000000000000000000000000002", n1, "ws-ci", "ci", true, None, None, now - 60);
        (victim, ci)
    };
    let mut c1 = compute_ws::test_attach(n1, solo_offer(1, 1));
    let mut c2 = compute_ws::test_attach(n2, solo_offer(1, 0));
    c1.frames();
    c2.frames();
    let looks = looks_for(&get(&ci.id), &k2_core::db::read(store::active_jobs), now);
    assert!(looks.iter().any(|l| l.node_id == n2 && l.starts_now), "{n2} starts it now: {looks:?}");
    sweep(now);
    assert!(cancels(&c1.frames()).is_empty(), "another node starts it now: nothing displaced");
    assert_eq!(get(&victim.id).reason, None);

    // n2 is busy now (a Batch job, not displaceable, no known end): the CI
    // job ends the victim on n1; the victim goes to n2's line, avoiding n1.
    {
        let conn = db.lock();
        let b = job(&conn, "d15d0000000000000000000000000003", n2, "ws-a", "batch", false, None, None, now - 50);
        running(&conn, &b.id, now - 40);
    }
    compute_ws::test_frame(n2, Frame::Offer(solo_offer(1, 1)));
    c2.frames();
    sweep(now + 1);
    assert_eq!(cancels(&c1.frames()), vec![(victim.id.clone(), FRAME_REASON.to_string())]);
    let before = get(&victim.id);
    stopped(n1, &before);
    let back = get(&victim.id);
    assert_eq!(back.generation, before.generation + 1);
    assert_eq!(back.attempt, before.attempt, "uncounted");
    assert_eq!(back.ctl.retries_used, 0);
    assert_eq!(back.node_id, n2, "it went elsewhere");
    let w = compute_place::Want::of_job(&back, &back.plan().expect("plan"));
    assert!(w.filter.avoids(n1, n1), "it avoids {n1} for 30 min");
    assert!(!w.filter.avoids(n2, n2));
    let words = queued_words(&back, now + 2).expect("words");
    assert!(words.contains(&format!("; avoiding {n1} for")), "{words}");
    // n1's next offer: the CI job (auto, queued for n1) starts there.
    compute_ws::test_frame(n1, Frame::Offer(solo_offer(1, 0)));
    assert_eq!(assigns(&c1.frames()).first(), Some(&ci.id), "the CI job takes the freed slot");
    compute_ws::test_detach(n1);
    compute_ws::test_detach(n2);
}
