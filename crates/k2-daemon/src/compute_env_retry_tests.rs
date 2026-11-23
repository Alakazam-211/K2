//! 0313: `k2 compute retry` runs a job with its own `--env` values or
//! refuses (`env_dropped`); never with the markers its plan keeps once
//! the values are gone. Kept apart so compute_routes.rs stays under its
//! line limit.

use super::*;
use k2_core::compute::proto::frames::{JobLimits, JobState, Plan};
use std::collections::BTreeMap;

fn plan(id: &str) -> Plan {
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
        argv: vec!["true".into()],
        env: BTreeMap::from([("BUNDLE_B64_1".to_string(), "QlVORExF".repeat(1024))]),
        cwd: None,
        limits: JobLimits { max_secs: 60, cpu_millis: None, mem_bytes: None, disk_bytes: 1, log_cap_bytes: 1 },
        src: None,
        exclusive: false,
        created_at: 100,
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
    }
}

fn ended(conn: &rusqlite::Connection, id: &str, state: JobState, at: i64) -> store::JobRow {
    let p = plan(id);
    store::insert_job(
        conn,
        &store::NewJob { plan: &p, client_job_id: id, session_id: None, request_sha256: "sha", detach: false, retry_interrupted: false, ctl: store::JobCtl::default() },
    )
    .expect("insert");
    store::set_job_state(conn, id, state, None, None, at).expect("end");
    store::job_by_id(conn, id).expect("row")
}

#[test]
fn retry_runs_with_the_env_inside_the_window_and_refuses_after_it() {
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    let now = compute::now();
    let failed = ended(&db.lock(), "j-fail", JobState::Failed, now);
    assert!(env_kept(&failed).is_ok(), "a failed job inside the retry window retries with its env");
    assert_eq!(failed.plan().unwrap().env["BUNDLE_B64_1"], plan("x").env["BUNDLE_B64_1"]);

    // A done job too (`k2 compute retry` of a done run is a real use).
    let done = ended(&db.lock(), "j-done", JobState::Done, now);
    assert!(env_kept(&done).is_ok(), "a done job inside the window retries with its env");

    // Past the window the trimmer drops the values: retry refuses.
    k2_core::compute::trim::trim_batch(now + store::ENV_KEEP_AFTER_END_SECS + 60).expect("trim");
    for id in ["j-fail", "j-done"] {
        let gone = store::job_by_id(&db.lock(), id).expect("row");
        let e = env_kept(&gone).expect_err("a retry past the window must refuse");
        assert_eq!((e.code, e.status), (codes::ENV_DROPPED, 409));
        assert!(e.hint.contains("7 days after it ended") && e.hint.contains("BUNDLE_B64_1") && e.hint.contains("submit it again"), "{}", e.hint);
    }
}
