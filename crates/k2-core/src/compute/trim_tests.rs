//! 0313: `--env` values live beside the plan while a job may run, ended
//! jobs keep a summary, and the trimmer never touches an open job.

use super::*;
use crate::compute::proto::frames::{JobLimits, JobState, Plan};
use crate::compute::store::{self, JobCtl, NewJob};
use std::collections::BTreeMap;

const DAY: i64 = 24 * 3600;
/// A git bundle passed as env, the way builders do (8 KB of base64).
fn bundle() -> String {
    "QlVORExFIGJ5dGVz".repeat(512)
}

fn plan(id: &str, env: &[(&str, &str)], created_at: i64) -> Plan {
    Plan {
        class: None,
        session_id: None,
        est_secs: None,
        est_p90_secs: None,
        ws_max_running: None,
        job_id: id.into(),
        node_id: "n1".into(),
        workspace_id: "ws".into(),
        workspace_label: "k2".into(),
        requested_by: "agent:k2".into(),
        argv: vec!["bash".into(), "-c".into(), format!("echo {}", "x".repeat(4000)), "tail".into()],
        env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        cwd: None,
        limits: JobLimits { max_secs: 60, cpu_millis: None, mem_bytes: None, disk_bytes: 1, log_cap_bytes: 1 },
        src: None,
        exclusive: false,
        created_at,
        needs: vec!["rust@1.99.1".into()],
        artifacts: Vec::new(),
        stall_secs: None,
        secret_env: Vec::new(),
        tier: None,
        cache: Some("warm".into()),
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
    }
}

fn submit(conn: &rusqlite::Connection, p: &Plan) -> store::JobRow {
    store::insert_job(
        conn,
        &NewJob { plan: p, client_job_id: &p.job_id, session_id: None, request_sha256: "sha", detach: false, retry_interrupted: false, ctl: JobCtl::default() },
    )
    .expect("insert")
}

fn raw_plan(conn: &rusqlite::Connection, id: &str) -> String {
    conn.query_row("SELECT plan_json FROM compute_jobs WHERE id = ?1", [id], |r| r.get(0)).expect("row")
}

fn node(conn: &rusqlite::Connection) {
    store::insert_pending_node(conn, "n1", "n1", "fp", "pem", &BTreeMap::new(), "owner", "1", 1).expect("node");
}

#[test]
fn a_submitted_job_keeps_no_env_value_in_its_plan_and_still_sends_it() {
    let _db = crate::db::scoped_for_test();
    let db = crate::db::shared();
    let conn = db.lock();
    node(&conn);
    let b = bundle();
    let p = plan("job-a", &[("BUNDLE_B64_1", &b), ("SECRETISH", "hunter2"), ("RUST_LOG", "info")], 100);
    let j = submit(&conn, &p);

    let raw = raw_plan(&conn, "job-a");
    assert!(!raw.contains(&b), "a BUNDLE_B64 value is in plan_json");
    assert!(!raw.contains("hunter2"), "an env value is in plan_json");
    assert!(raw.contains("BUNDLE_B64_1") && raw.contains("k2redacted"), "keys and markers stay: {raw}");
    assert!(raw.contains(&format!("\"bytes\\\":{}", b.len())), "the marker gives the size: {raw}");
    assert_eq!(store::job_env_rows(&conn, "job-a"), 3);
    // What it is sent with: the real values, and the digest is theirs.
    let back = j.plan().expect("plan");
    assert_eq!(back.env, p.env, "the row carries the values again");
    assert_eq!(back.digest().unwrap(), j.plan_digest, "the plan digest is over the real values");
    assert_eq!(back, p);
}

#[test]
fn a_queued_job_survives_a_restart_with_its_env() {
    let dir = std::env::temp_dir().join(format!("k2-trim-restart-{}-{}", std::process::id(), crate::compute::now()));
    std::fs::create_dir_all(&dir).expect("dir");
    struct Gone(std::path::PathBuf);
    impl Drop for Gone {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _gone = Gone(dir.clone());
    let path = dir.join("k2.db");
    let b = bundle();
    let p = plan("job-r", &[("BUNDLE_B64_1", &b)], 100);
    {
        let conn = crate::db::open_with_resilience(&path).expect("open");
        crate::db::run_migrations(&conn).expect("migrate");
        node(&conn);
        submit(&conn, &p);
    }
    // A new process: a new connection on the same file.
    let conn = crate::db::open_with_resilience(&path).expect("reopen");
    let j = store::job_by_id(&conn, "job-r").expect("job");
    assert_eq!(j.state, "queued");
    let back = j.plan().expect("plan");
    assert_eq!(back.env.get("BUNDLE_B64_1"), Some(&b), "the env value is there after the restart");
    assert_eq!(back.digest().unwrap(), j.plan_digest, "it dispatches with the digest it was submitted with");
    // A move rewrites the plan (new node, new digest): the values go along.
    let moved = Plan { node_id: "n2".into(), ..back };
    assert!(store::move_queued_job(&conn, "job-r", &moved, &JobCtl::default(), "test").expect("move"));
    let j = store::job_by_id(&conn, "job-r").expect("job");
    assert_eq!(j.plan().expect("plan").env.get("BUNDLE_B64_1"), Some(&b));
    assert_eq!(j.plan().unwrap().digest().unwrap(), j.plan_digest);
    assert!(!raw_plan(&conn, "job-r").contains(&b));
}

#[test]
fn an_ended_job_keeps_its_env_for_the_window_then_the_trimmer_drops_it() {
    let _db = crate::db::scoped_for_test();
    let db = crate::db::shared();
    let conn = db.lock();
    node(&conn);
    for (id, state) in [("j-done", JobState::Done), ("j-cancel", JobState::Cancelled), ("j-fail", JobState::Failed)] {
        submit(&conn, &plan(id, &[("A", "1"), ("B", "2")], 100));
        store::set_job_state(&conn, id, state, None, None, 200).expect("end");
        // Kept: a retry, the receipt the node sends after the end and
        // `k2 compute receipt` all need the values.
        assert_eq!(store::job_env_rows(&conn, id), 2, "{id} keeps its env rows when it ends");
        let j = store::job_by_id(&conn, id).unwrap();
        assert_eq!(j.plan().unwrap().digest().unwrap(), j.plan_digest, "{id}: the plan still hashes to its digest");
    }
    // A retry inside the window runs with the values.
    let r = store::requeue_new_attempt(&conn, "j-fail", "retry").expect("retry");
    assert_eq!(r.plan().unwrap().env.get("A").map(String::as_str), Some("1"));
    store::set_job_state(&conn, "j-fail", JobState::Failed, None, None, 300).expect("fails again");
    drop(conn);
    // Inside the window the trimmer leaves them; past it they go.
    trim_batch(300 + store::ENV_KEEP_AFTER_END_SECS - 10).expect("batch");
    assert_eq!(store::job_env_rows(&db.lock(), "j-fail"), 2, "kept inside the window");
    assert_eq!(store::job_env_rows(&db.lock(), "j-done"), 0, "the done job ended at 200: past its window");
    assert_eq!(store::job_env_rows(&db.lock(), "j-cancel"), 0);
    trim_batch(300 + store::ENV_KEEP_AFTER_END_SECS + 10).expect("batch");
    assert_eq!(store::job_env_rows(&db.lock(), "j-fail"), 0, "dropped past the window");
    let p = store::job_by_id(&db.lock(), "j-fail").unwrap().plan().unwrap();
    assert_eq!(store::env_dropped(&p).len(), 2, "after the window the plan has markers only: a retry must refuse");
}

#[test]
fn signed_evidence_keeps_its_env_after_it_ends() {
    let _db = crate::db::scoped_for_test();
    let db = crate::db::shared();
    let conn = db.lock();
    node(&conn);
    let p = Plan { receipt_v: Some(2), ..plan("j-rel", &[("A", "1")], 100) };
    submit(&conn, &p);
    store::set_job_state(&conn, "j-rel", JobState::Done, None, None, 200).expect("end");
    assert_eq!(store::job_env_rows(&conn, "j-rel"), 1, "a receipt v2 job's plan is re-hashed by a release");
    let j = store::job_by_id(&conn, "j-rel").unwrap();
    assert_eq!(j.plan().unwrap().digest().unwrap(), j.plan_digest);
    drop(conn);
    trim_batch(200 + TRIM_AFTER_SECS + DAY).expect("batch");
    let j = store::job_by_id(&db.lock(), "j-rel").unwrap();
    assert_eq!(j.plan().unwrap().digest().unwrap(), j.plan_digest, "the trimmer leaves signed evidence whole");
}

#[test]
fn the_trimmer_trims_old_ended_plans_leaves_open_jobs_and_resumes() {
    let _db = crate::db::scoped_for_test();
    let db = crate::db::shared();
    let b = bundle();
    let ended = TRIM_BATCH + 50;
    {
        let conn = db.lock();
        node(&conn);
        // Old jobs written before 0313 (values in the plan), ended long ago.
        for i in 0..ended {
            let id = format!("old-{i:04}");
            let p = plan(&id, &[("BUNDLE_B64_1", &b)], 1000 + i as i64);
            let j = submit(&conn, &p);
            let legacy = serde_json::to_string(&p).unwrap();
            conn.execute("UPDATE compute_jobs SET plan_json = ?2 WHERE id = ?1", rusqlite::params![j.id, legacy]).unwrap();
            store::set_job_state(&conn, &id, JobState::Done, None, None, 2000).expect("end");
        }
        // Open jobs just as old: queued, running, and node-queued.
        for (i, state) in ["queued", "running", "assigned"].iter().enumerate() {
            let id = format!("open-{i}");
            submit(&conn, &plan(&id, &[("BUNDLE_B64_1", &b)], 500 + i as i64));
            conn.execute("UPDATE compute_jobs SET state = ?2 WHERE id = ?1", rusqlite::params![id, state]).unwrap();
        }
    }
    let now = 2000 + TRIM_AFTER_SECS + DAY;
    let open_before: Vec<String> = (0..3).map(|i| raw_plan(&db.lock(), &format!("open-{i}"))).collect();

    // One batch, then a "restart": the place was saved.
    let first = trim_batch(now).expect("batch 1");
    assert_eq!(first.trimmed, TRIM_BATCH, "{first:?}");
    assert!(!first.done);
    let saved = cursor(&db.lock());
    assert_eq!(saved, (1000 + TRIM_BATCH as i64 - 1, format!("old-{:04}", TRIM_BATCH - 1)), "the place is saved with the batch");
    let second = trim_batch(now).expect("batch 2");
    assert_eq!(second.trimmed, ended - TRIM_BATCH, "it resumes where it stopped: {second:?}");
    let third = trim_batch(now).expect("batch 3");
    assert!(third.done && third.trimmed == 0, "{third:?}");

    let conn = db.lock();
    for i in 0..ended {
        let raw = raw_plan(&conn, &format!("old-{i:04}"));
        assert!(raw.len() < 3000, "trimmed to a summary ({} bytes)", raw.len());
        assert!(!raw.contains(&b));
        let p: Plan = serde_json::from_str(&raw).expect("a trimmed plan still reads");
        assert_eq!((p.cache.as_deref(), p.needs.clone()), (Some("warm"), vec!["rust@1.99.1".to_string()]), "cache and needs stay");
        assert!(p.env.contains_key("BUNDLE_B64_1"), "env keys stay");
        let argv_bytes: usize = p.argv.iter().map(String::len).sum();
        assert!(argv_bytes <= ARGV_KEEP_BYTES + 64, "argv kept to about 1 KB ({argv_bytes})");
    }
    for (i, before) in open_before.iter().enumerate() {
        assert_eq!(&raw_plan(&conn, &format!("open-{i}")), before, "an open job is never trimmed");
        assert_eq!(store::job_env_rows(&conn, &format!("open-{i}")), 1, "an open job keeps its env");
    }
    // A plan already trimmed is left alone.
    assert_eq!(trim_plan_json(&raw_plan(&conn, "old-0000"), now), None);
}
