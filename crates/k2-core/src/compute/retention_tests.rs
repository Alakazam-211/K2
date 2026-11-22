//! The retention rule (0.46.17): age, cap, pins, running jobs, files in
//! use, orphans; the policy and pins round-trip; the scan names each file.

use super::*;

const DAY: i64 = 86_400;
const NOW: i64 = 100 * DAY;
const GB: u64 = 1 << 30;

fn art(job: &str, gen: i64, bytes: u64, mtime: i64) -> Entry {
    Entry { path: PathBuf::from(format!("/c/artifacts/{job}-g{gen}")), kind: Kind::Artifacts, job: Some(job.into()), generation: Some(gen), bytes, files: 1, mtime }
}

fn dirty(job: &str, bytes: u64, mtime: i64) -> Entry {
    Entry { path: PathBuf::from(format!("/c/blobs/{job}.dirty.tar")), kind: Kind::Dirty, job: Some(job.into()), generation: None, bytes, files: 1, mtime }
}

fn bundle(job: &str, gen: i64, mtime: i64) -> Entry {
    Entry { path: PathBuf::from(format!("/c/blobs/{job}-g{gen}.bundle")), kind: Kind::Bundle, job: Some(job.into()), generation: Some(gen), bytes: 10, files: 1, mtime }
}

fn ended(at: i64) -> JobFact {
    JobFact { ended: true, ended_at: Some(at), parent: None, workspace_id: "ws".into(), only_copy: false }
}

fn running() -> JobFact {
    JobFact { ended: false, ended_at: None, parent: None, workspace_id: "ws".into(), only_copy: false }
}

#[test]
fn an_only_copy_is_never_taken_for_the_cap_but_still_goes_by_age() {
    let only = |at: i64| JobFact { only_copy: true, ..ended(at) };
    let facts: HashMap<String, JobFact> =
        [("only".to_string(), only(NOW - 3 * DAY)), ("plain".to_string(), ended(NOW - DAY)), ("oldonly".to_string(), only(NOW - 8 * DAY))].into();
    let p = run(vec![art("only", 1, 3 * GB, NOW - 3 * DAY), art("plain", 1, GB, NOW - DAY), art("oldonly", 1, GB, NOW - 8 * DAY)], &facts, &[], &never, 2 * GB);
    assert_eq!(
        removed(&p),
        vec![("oldonly".to_string(), Why::Age), ("plain".to_string(), Why::Cap)],
        "the older only copy is skipped for the cap and the younger plain job goes instead; an only copy past the max age goes"
    );
    assert_eq!(kept(&p, "only"), Keep::OnlyCopy);
    assert_eq!(p.only_copy_bytes(), 3 * GB);
    assert!(p.over_cap(), "only copies alone fill the cap: the plan says so, it deletes nothing for it");
}

fn never(_: &Entry) -> bool {
    false
}

fn run(entries: Vec<Entry>, facts: &HashMap<String, JobFact>, pins: &[&str], in_use: &dyn Fn(&Entry) -> bool, cap: u64) -> Plan {
    let pins: BTreeSet<String> = pins.iter().map(|p| p.to_string()).collect();
    plan(Input { entries, facts, pins: &pins, in_use, now: NOW, max_age: 7 * DAY, cap })
}

fn removed(p: &Plan) -> Vec<(String, Why)> {
    let mut v: Vec<(String, Why)> = p.remove.iter().map(|(u, w)| (u.job.clone().unwrap_or_else(|| u.entries[0].path.display().to_string()), *w)).collect();
    v.sort();
    v
}

fn kept(p: &Plan, job: &str) -> Keep {
    p.keep.iter().find(|(u, _)| u.job.as_deref() == Some(job)).map(|(_, k)| *k).unwrap_or_else(|| panic!("{job} not kept: {:?}", removed(p)))
}

#[test]
fn a_job_past_the_max_age_goes_with_its_blob_and_a_young_one_stays() {
    let facts: HashMap<String, JobFact> = [("old".to_string(), ended(NOW - 8 * DAY)), ("new".to_string(), ended(NOW - DAY))].into();
    let p = run(vec![art("old", 1, GB, NOW - 8 * DAY), dirty("old", 5, NOW - 9 * DAY), art("new", 1, GB, NOW - DAY), dirty("new", 5, NOW - DAY)], &facts, &[], &never, 100 * GB);
    assert_eq!(removed(&p), vec![("old".to_string(), Why::Age)]);
    let old = &p.remove[0].0;
    assert_eq!(old.entries.len(), 2, "the blob goes with the artifacts of the job that referenced it");
    assert_eq!(kept(&p, "new"), Keep::Young);
    assert_eq!(p.bytes_after(), GB + 5);
}

#[test]
fn the_age_counts_from_the_job_end_not_the_file_time() {
    // Fetched yesterday, but the job ended 8 days ago: it goes.
    let facts: HashMap<String, JobFact> = [("j".to_string(), ended(NOW - 8 * DAY))].into();
    let p = run(vec![art("j", 1, 1, NOW - DAY)], &facts, &[], &never, 100 * GB);
    assert_eq!(removed(&p), vec![("j".to_string(), Why::Age)]);
}

#[test]
fn over_the_cap_the_oldest_ended_jobs_go_first() {
    let facts: HashMap<String, JobFact> = [("a".to_string(), ended(NOW - 3 * DAY)), ("b".to_string(), ended(NOW - 2 * DAY)), ("c".to_string(), ended(NOW - DAY))].into();
    let p = run(vec![art("a", 1, 4 * GB, NOW - 3 * DAY), art("b", 1, 4 * GB, NOW - 2 * DAY), art("c", 1, 4 * GB, NOW - DAY)], &facts, &[], &never, 5 * GB);
    assert_eq!(removed(&p), vec![("a".to_string(), Why::Cap), ("b".to_string(), Why::Cap)], "oldest first until under 5 GB");
    assert_eq!(kept(&p, "c"), Keep::Young);
    assert!(!p.over_cap());
}

#[test]
fn a_pinned_job_survives_age_and_cap_and_a_pinned_parent_keeps_its_shards() {
    let mut shard = ended(NOW - 30 * DAY);
    shard.parent = Some("parent".into());
    let facts: HashMap<String, JobFact> = [("pin".to_string(), ended(NOW - 30 * DAY)), ("shard1".to_string(), shard), ("parent".to_string(), ended(NOW - 30 * DAY))].into();
    let p = run(vec![art("pin", 1, 10 * GB, NOW - 30 * DAY), dirty("pin", 1, NOW - 30 * DAY), art("shard1", 1, GB, NOW - 30 * DAY)], &facts, &["pin", "parent"], &never, GB);
    assert!(p.remove.is_empty(), "{:?}", removed(&p));
    assert_eq!(kept(&p, "pin"), Keep::Pinned);
    assert_eq!(kept(&p, "shard1"), Keep::Pinned);
    assert!(p.over_cap(), "pinned bytes alone fill the cap, and the plan says so");
}

#[test]
fn a_running_job_keeps_its_blob_but_not_a_stale_bundle() {
    let facts: HashMap<String, JobFact> = [("run".to_string(), running())].into();
    let p = run(vec![dirty("run", 50, NOW - 10 * DAY), bundle("run", 1, NOW - 2 * 3600), bundle("run", 2, NOW - 60)], &facts, &[], &never, 1);
    assert_eq!(removed(&p), vec![("run".to_string(), Why::Leftover)]);
    assert_eq!(p.remove[0].0.entries, vec![bundle("run", 1, NOW - 2 * 3600)], "only the bundle past the grace");
    let live = p.keep.iter().find(|(u, _)| u.job.as_deref() == Some("run")).expect("kept");
    assert_eq!(live.1, Keep::Active);
    assert_eq!(live.0.entries.len(), 2, "its dirty blob and the fresh bundle stay, cap or not");
}

#[test]
fn an_entry_in_use_is_never_removed() {
    let facts: HashMap<String, JobFact> = [("busy".to_string(), ended(NOW - 30 * DAY))].into();
    let busy = |e: &Entry| e.job.as_deref() == Some("busy") && e.kind == Kind::Artifacts;
    let p = run(vec![art("busy", 1, GB, NOW - 30 * DAY), dirty("busy", 1, NOW - 30 * DAY)], &facts, &[], &busy, 1);
    assert!(p.remove.is_empty(), "{:?}", removed(&p));
    assert_eq!(kept(&p, "busy"), Keep::InUse);
}

#[test]
fn files_of_unknown_jobs_go_only_after_the_grace() {
    let facts = HashMap::new();
    let other = Entry { path: PathBuf::from("/c/blobs/plan-abc.dirty.tar"), kind: Kind::Other, job: None, generation: None, bytes: 3, files: 1, mtime: NOW - 2 * 3600 };
    let p = run(vec![dirty("submitting", 5, NOW - 30), art("gone", 1, 5, NOW - 2 * 3600), other], &facts, &[], &never, 100 * GB);
    let r = removed(&p);
    assert_eq!(r, vec![("/c/blobs/plan-abc.dirty.tar".to_string(), Why::Orphan), ("gone".to_string(), Why::Orphan)]);
    assert_eq!(kept(&p, "submitting"), Keep::Young, "a blob written a moment before its job row stays");
}

#[test]
fn the_default_cap_is_20_gb_or_10_percent_of_the_disk() {
    assert_eq!(default_cap(0), 20 * GB, "unknown disk");
    assert_eq!(default_cap(1000 * GB), 20 * GB);
    assert_eq!(default_cap(128 * GB), 128 * GB / 100 * 10);
    assert_eq!(default_cap(4 * GB), MIN_CAP_BYTES, "never below 1 GB");
    let p = Policy::default();
    assert_eq!((p.max_age(), p.cap(1000 * GB), p.cap_from()), (7 * DAY, 20 * GB, "auto"));
}

#[test]
fn durations_and_sizes_parse_and_bad_ones_are_refused() {
    assert_eq!(parse_duration("3d"), Ok(3 * DAY));
    assert_eq!(parse_duration("1d12h"), Ok(DAY + 12 * 3600));
    assert_eq!(parse_duration("90m"), Ok(5400));
    assert_eq!(parse_duration("0"), Ok(0));
    for bad in ["", "3x", "d", "-1", "3d4"] {
        assert!(parse_duration(bad).is_err(), "{bad:?}");
    }
    assert_eq!(parse_size("20G"), Ok(20 * GB));
    assert_eq!(parse_size("512M"), Ok(512 << 20));
    assert_eq!(parse_size("1T"), Ok(1 << 40));
    assert!(parse_size("lots").is_err());
}

#[test]
fn the_policy_and_pins_round_trip_and_a_bad_policy_is_refused() {
    let conn = crate::db::isolated_test_connection();
    assert_eq!(load(&conn), Policy::default());
    let p = Policy { max_age_secs: Some(3 * DAY), cap_bytes: Some(5 * GB) };
    save(&conn, &p, "owner", 1).expect("save");
    assert_eq!(load(&conn), p);
    assert!(save(&conn, &Policy { cap_bytes: Some(1), ..Policy::default() }, "owner", 2).is_err(), "a cap under 1 GB");
    assert!(save(&conn, &Policy { max_age_secs: Some(60), ..Policy::default() }, "owner", 2).is_err(), "a max age under 1 h");
    assert_eq!(load(&conn), p, "a refused save changes nothing");
    save(&conn, &Policy::default(), "owner", 3).expect("back to the default");
    assert_eq!(load(&conn), Policy::default());

    assert_eq!(set_pin(&conn, "job1", true, "owner", 5), Ok(true));
    assert_eq!(set_pin(&conn, "job1", true, "owner", 6), Ok(false), "already pinned");
    let all = pins(&conn).expect("pins");
    assert_eq!(all.get("job1"), Some(&Pin { job: "job1".into(), by: "owner".into(), at: 5 }));
    assert_eq!(set_pin(&conn, "job1", false, "owner", 7), Ok(true));
    assert_eq!(set_pin(&conn, "job1", false, "owner", 8), Ok(false));
    assert!(pins(&conn).expect("pins").is_empty());
}

#[test]
fn the_scan_names_each_file_and_measures_it() {
    let root = std::env::temp_dir().join(format!("k2-retention-scan-{}-{}", std::process::id(), crate::compute::now()));
    struct Gone(PathBuf);
    impl Drop for Gone {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _g = Gone(root.clone());
    let job = "0123456789abcdef0123456789abcdef";
    std::fs::create_dir_all(root.join(format!("artifacts/{job}-g2"))).unwrap();
    std::fs::write(root.join(format!("artifacts/{job}-g2/aa")), vec![1u8; 100]).unwrap();
    std::fs::write(root.join(format!("artifacts/{job}-g2/.bb.part")), vec![1u8; 20]).unwrap();
    std::fs::create_dir_all(root.join("blobs")).unwrap();
    std::fs::write(root.join(format!("blobs/{job}.dirty.tar")), vec![1u8; 7]).unwrap();
    std::fs::write(root.join(format!("blobs/{job}-g3.bundle")), vec![1u8; 9]).unwrap();
    std::fs::write(root.join("blobs/plan-xyz.dirty.tar"), vec![1u8; 4]).unwrap();
    let got: Vec<(Kind, Option<String>, Option<i64>, u64, u64)> = scan(&root).into_iter().map(|e| (e.kind, e.job, e.generation, e.bytes, e.files)).collect();
    assert_eq!(
        got,
        vec![
            (Kind::Artifacts, Some(job.into()), Some(2), 120, 2),
            (Kind::Bundle, Some(job.into()), Some(3), 9, 1),
            (Kind::Dirty, Some(job.into()), None, 7, 1),
            (Kind::Other, None, None, 4, 1),
        ]
    );
}

#[test]
fn job_facts_read_the_end_and_the_shard_parent() {
    use crate::compute::proto::frames::JobState;
    let _db = crate::db::scoped_for_test();
    let db = crate::db::shared();
    let conn = db.lock();
    crate::compute::store::insert_pending_node(&conn, "n1", "n1", "fp", "pem", &BTreeMap::new(), "owner", "1", 1).expect("node");
    store_job(&conn, "jobend", None);
    store_job(&conn, "jobkid", Some("jobend"));
    crate::compute::store::set_job_state(&conn, "jobend", JobState::Done, None, None, 1234).expect("end");
    let ids: BTreeSet<String> = ["jobend", "jobkid", "jobnone"].iter().map(|s| s.to_string()).collect();
    let f = job_facts(&conn, &ids);
    assert_eq!(f.get("jobend"), Some(&JobFact { ended: true, ended_at: Some(1234), parent: None, workspace_id: "ws".into(), only_copy: false }));
    assert_eq!(f.get("jobkid").map(|k| (k.ended, k.parent.clone())), Some((false, Some("jobend".into()))));
    // The secret fetch-then-delete's event marks the only copy; a failed one doesn't.
    crate::compute::store::record_event(&conn, "controller", ONLY_COPY_EVENT, None, None, Some("jobend"), serde_json::json!({"files": 1, "deleted": true}));
    crate::compute::store::record_event(&conn, "controller", ONLY_COPY_EVENT, None, None, Some("jobkid"), serde_json::json!({"deleted": false, "code": "node_offline"}));
    let f = job_facts(&conn, &ids);
    assert_eq!((f["jobend"].only_copy, f["jobkid"].only_copy), (true, false));
    assert!(!f.contains_key("jobnone"));
}

fn store_job(conn: &Connection, id: &str, parent: Option<&str>) {
    use crate::compute::proto::frames::{JobLimits, Plan};
    use crate::compute::store::{self, JobCtl, NewJob, ShardRef};
    let p = Plan {
        class: None,
        session_id: None,
        est_secs: None,
        est_p90_secs: None,
        ws_max_running: None,
        job_id: id.into(),
        node_id: "n1".into(),
        workspace_id: "ws".into(),
        workspace_label: "k2".into(),
        requested_by: "owner".into(),
        argv: vec!["true".into()],
        env: BTreeMap::new(),
        cwd: None,
        limits: JobLimits { max_secs: 60, cpu_millis: None, mem_bytes: None, disk_bytes: 1, log_cap_bytes: 1 },
        src: None,
        exclusive: false,
        created_at: 1,
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
        with: Vec::new(),
    };
    let ctl = JobCtl { shard: parent.map(|p| ShardRef { parent: p.into(), index: 1, of: 2 }), ..JobCtl::default() };
    store::insert_job(conn, &NewJob { plan: &p, client_job_id: id, session_id: None, request_sha256: "h", detach: false, retry_interrupted: false, ctl }).expect("insert");
}
