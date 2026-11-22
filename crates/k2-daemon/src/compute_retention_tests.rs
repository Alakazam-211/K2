//! The compute store's retention on real files (0.46.17): age and cap
//! pruning, blobs kept while their job is, a prune during a fetch, pins,
//! dry runs, who may prune, and the POST-only route table.

use super::*;
use k2_core::compute::proto::frames::{self, ArtifactChunk, Frame, JobLimits, JobState, Offer, Plan};
use k2_core::compute::proto::crypto;
use k2_core::compute::proto::results::{self, ArtifactInfo};
use std::collections::BTreeMap;

const DAY: i64 = 86_400;

fn node(conn: &rusqlite::Connection, id: &str) {
    store::insert_pending_node(conn, id, id, &format!("fp-{id}"), "pem", &BTreeMap::new(), "owner", "123456", 1).expect("node");
}

/// A job on `node_id`, ended `ended_ago` seconds before now (`None`: still running).
fn job(conn: &rusqlite::Connection, id: &str, node_id: &str, ended_ago: Option<i64>, dirty: bool) -> store::JobRow {
    let src = dirty.then(|| frames::Src {
        project_key: "pk".into(),
        remote_url: None,
        commit: "c".repeat(40),
        dirty: Some(frames::Blob { sha256: "d".repeat(64), bytes: 5 }),
        slots: 1,
    });
    let plan = Plan {
        class: None,
        session_id: None,
        est_secs: None,
        est_p90_secs: None,
        ws_max_running: None,
        job_id: id.into(),
        node_id: node_id.into(),
        workspace_id: "ws".into(),
        workspace_label: "ws".into(),
        requested_by: "owner".into(),
        argv: vec!["true".into()],
        env: BTreeMap::new(),
        cwd: None,
        limits: JobLimits { max_secs: 60, cpu_millis: None, mem_bytes: None, disk_bytes: 1, log_cap_bytes: 1 },
        src,
        exclusive: false,
        created_at: 10,
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
    store::insert_job(
        conn,
        &store::NewJob { plan: &plan, client_job_id: id, session_id: None, request_sha256: "h", detach: false, retry_interrupted: false, ctl: Default::default() },
    )
    .expect("insert job");
    if let Some(ago) = ended_ago {
        store::set_job_state(conn, id, JobState::Done, None, None, compute::now() - ago).expect("end");
    } else {
        assert!(store::assign_if_queued(conn, id, compute::now()).expect("assign"));
        store::set_job_state(conn, id, JobState::Running, None, None, compute::now()).expect("run");
    }
    store::job_by_id(conn, id).expect("row")
}

/// `bytes` of artifact in the job's folder (sparse: big sizes cost no disk).
fn put_artifact(j: &store::JobRow, bytes: u64) -> PathBuf {
    let dir = artifacts_dir(&j.id, j.generation);
    std::fs::create_dir_all(&dir).expect("dir");
    let f = std::fs::File::create(dir.join("e".repeat(64))).expect("file");
    f.set_len(bytes).expect("size");
    dir
}

fn put_dirty(job_id: &str) -> PathBuf {
    let p = crate::compute_ws::dirty_blob_path(job_id);
    std::fs::create_dir_all(p.parent().unwrap()).expect("blobs");
    std::fs::write(&p, b"dirty").expect("blob");
    p
}

/// Set a file or folder's mtime `ago` seconds back.
fn age(p: &Path, ago: i64) {
    let t = std::time::SystemTime::now() - Duration::from_secs(ago as u64);
    let f = std::fs::File::options().read(true).open(p).expect("open");
    f.set_modified(t).expect("mtime");
}

/// No grace after a fetch: only a live holder counts as in use.
fn opts(older_than: Option<i64>, dry_run: bool) -> Opts {
    Opts { older_than, dry_run, grace: Duration::ZERO, now: compute::now() }
}

fn ids(v: &[(rt::Unit, Why)]) -> Vec<(String, Why)> {
    let mut out: Vec<(String, Why)> = v.iter().map(|(u, w)| (u.job.clone().unwrap_or_default(), *w)).collect();
    out.sort();
    out
}

#[test]
fn a_job_past_the_max_age_goes_with_its_blob_and_young_and_running_ones_stay() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    let (old, young, live) = {
        let conn = db.lock();
        node(&conn, "n-age");
        (
            job(&conn, "a01d0000000000000000000000000000", "n-age", Some(8 * DAY), true),
            job(&conn, "a02d0000000000000000000000000000", "n-age", Some(DAY), true),
            job(&conn, "a03d0000000000000000000000000000", "n-age", None, true),
        )
    };
    let old_dir = put_artifact(&old, 10);
    let old_blob = put_dirty(&old.id);
    let young_dir = put_artifact(&young, 10);
    let young_blob = put_dirty(&young.id);
    // A running job's blob, written long ago: it may be sent again.
    let live_blob = put_dirty(&live.id);
    age(&live_blob, 30 * DAY);

    let o = prune(&opts(None, false)).expect("prune");
    assert_eq!(ids(&o.removed), vec![(old.id.clone(), Why::Age)]);
    assert!(!old_dir.exists() && !old_blob.exists(), "the old job's files and its blob are gone");
    assert!(young_dir.exists() && young_blob.exists(), "a job that ended yesterday keeps its files and blob");
    assert!(live_blob.exists(), "a running job keeps its blob");
    assert!(!compute::compute_dir().join(rt::TRASH_DIR).exists(), "nothing is left in the trash");

    // --older-than takes younger jobs once; the running one still stays.
    let o = prune(&opts(Some(3600), false)).expect("prune");
    assert_eq!(ids(&o.removed), vec![(young.id.clone(), Why::Age)]);
    assert!(!young_dir.exists() && !young_blob.exists());
    assert!(live_blob.exists(), "--older-than never takes a running job's blob");
}

#[test]
fn over_the_cap_the_oldest_ended_jobs_go_first() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    let jobs: Vec<store::JobRow> = {
        let conn = db.lock();
        node(&conn, "n-cap");
        rt::save(&conn, &rt::Policy { cap_bytes: Some(1 << 30), ..Default::default() }, "owner", 1).expect("cap 1 GB");
        (0..3).map(|i| job(&conn, &format!("cab{i}0000000000000000000000000000"), "n-cap", Some((3 - i) * DAY), false)).collect()
    };
    // 3 × 600 MB (sparse) against a 1 GB cap: the two oldest go.
    let dirs: Vec<PathBuf> = jobs.iter().map(|j| put_artifact(j, 600 << 20)).collect();
    let o = prune(&opts(None, false)).expect("prune");
    assert_eq!(ids(&o.removed), vec![(jobs[0].id.clone(), Why::Cap), (jobs[1].id.clone(), Why::Cap)]);
    assert!(!dirs[0].exists() && !dirs[1].exists());
    assert!(dirs[2].exists(), "the newest stays: the store is under the cap");
    assert_eq!(o.bytes_after(), 600 << 20);
}

/// A secret job's files the node deleted after this server fetched them
/// are the only copy: the cap passes them over (a younger job goes
/// instead), `retention` lists them with the date they go, the store
/// line says they alone hold it over the cap, and the age rule still
/// takes them.
#[test]
fn only_copies_are_kept_from_the_cap_listed_and_still_go_by_age() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    let (only, plain) = {
        let conn = db.lock();
        node(&conn, "n-only");
        rt::save(&conn, &rt::Policy { cap_bytes: Some(1 << 30), ..Default::default() }, "owner", 1).expect("cap 1 GB");
        let only = job(&conn, "0c0c0000000000000000000000000000", "n-only", Some(3 * DAY), false);
        store::record_event(&conn, "controller", rt::ONLY_COPY_EVENT, Some("n-only"), Some("ws"), Some(&only.id), serde_json::json!({"files": 1, "deleted": true}));
        (only, job(&conn, "0d0d0000000000000000000000000000", "n-only", Some(DAY), false))
    };
    // 1.5 GB only copy (the oldest) + 600 MB plain job, against 1 GB.
    let only_dir = put_artifact(&only, 1536 << 20);
    let plain_dir = put_artifact(&plain, 600 << 20);
    let o = prune(&opts(None, false)).expect("prune");
    assert_eq!(ids(&o.removed), vec![(plain.id.clone(), Why::Cap)], "the plain job goes; the older only copy is passed over");
    assert!(only_dir.exists() && !plain_dir.exists());
    assert!(o.kept.iter().any(|(u, k)| u.job.as_deref() == Some(only.id.as_str()) && *k == Keep::OnlyCopy));
    let text = outcome_json(&o, compute::now())["text"].to_string();
    assert!(text.contains("over the cap: 1.5 GB held as only copies"), "{text}");

    let line = summary()["line"].as_str().expect("line").to_string();
    assert!(line.contains("over the cap: 1.5 GB held as only copies"), "{line}");
    assert!(!line.contains("OVER THE CAP"), "only copies alone: the only-copy wording, not the prune hint: {line}");
    let v = handle(RETENTION, &Who::Owner, b"").expect("retention");
    let until = only.ended_at.expect("ended") + 7 * DAY;
    let want = format!("0c0c0000 1.5 GB: only copy, kept until {}", utc_date(until));
    assert!(v["text"].as_array().expect("text").iter().any(|l| l.as_str().map(str::trim) == Some(want.as_str())), "{want} in {}", v["text"]);

    // Past the max age it goes like any other job.
    let o = prune(&opts(Some(2 * DAY), false)).expect("prune by age");
    assert_eq!(ids(&o.removed), vec![(only.id.clone(), Why::Age)]);
    assert!(!only_dir.exists());
}

#[test]
fn pinned_jobs_survive_and_unpinned_they_go() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    let j = {
        let conn = db.lock();
        node(&conn, "n-pin");
        let j = job(&conn, "b1b00000000000000000000000000000", "n-pin", Some(30 * DAY), true);
        rt::set_pin(&conn, &j.id, true, "owner", 1).expect("pin");
        j
    };
    let dir = put_artifact(&j, 10);
    let blob = put_dirty(&j.id);
    let o = prune(&opts(Some(0), false)).expect("prune");
    assert!(o.removed.is_empty(), "{:?}", ids(&o.removed));
    assert!(o.kept.iter().any(|(u, k)| u.job.as_deref() == Some(j.id.as_str()) && *k == Keep::Pinned));
    assert!(dir.exists() && blob.exists(), "a pinned job keeps its files and its blob");
    rt::set_pin(&db.lock(), &j.id, false, "owner", 2).expect("unpin");
    let o = prune(&opts(None, false)).expect("prune");
    assert_eq!(ids(&o.removed), vec![(j.id.clone(), Why::Age)]);
    assert!(!dir.exists() && !blob.exists());
}

#[test]
fn a_dry_run_deletes_nothing_and_says_what_would_go() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    let j = {
        let conn = db.lock();
        node(&conn, "n-dry");
        job(&conn, "d1d00000000000000000000000000000", "n-dry", Some(30 * DAY), true)
    };
    let dir = put_artifact(&j, 4096);
    let blob = put_dirty(&j.id);
    let o = prune(&opts(None, true)).expect("dry run");
    assert_eq!(ids(&o.removed), vec![(j.id.clone(), Why::Age)]);
    assert!(dir.exists() && blob.exists(), "a dry run deletes nothing");
    let v = outcome_json(&o, compute::now());
    assert_eq!(v["dryRun"], true);
    assert_eq!(v["removed"]["bytes"], 4096 + 5);
    assert_eq!((v["removed"]["artifactFolders"].as_u64(), v["removed"]["blobFiles"].as_u64()), (Some(1), Some(1)));
    let text = v["text"].as_array().expect("text").iter().map(|l| l.as_str().expect("line")).collect::<Vec<_>>().join("\n");
    assert!(text.contains("would remove 1 job(s)' files") && text.contains("nothing was deleted (--dry-run)"), "{text}");
}

#[test]
fn files_whose_job_is_unknown_go_after_the_grace_and_a_fresh_submit_blob_stays() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let fresh = put_dirty("f1f00000000000000000000000000000");
    let stale = put_dirty("f2f00000000000000000000000000000");
    age(&stale, 2 * 3600);
    let o = prune(&opts(None, false)).expect("prune");
    assert_eq!(ids(&o.removed), vec![("f2f00000000000000000000000000000".to_string(), Why::Orphan)]);
    assert!(fresh.exists(), "a blob written a moment before its job row stays");
    assert!(!stale.exists());
}

/// A node offer with `features` and `version`.
fn offer(features: &[&str], version: &str) -> Offer {
    Offer {
        share: None,
        revision: 1,
        control: frames::Control::Active,
        control_by: None,
        control_at: None,
        availability: frames::Availability { ok: true, reasons: vec![] },
        caps: frames::Caps { cpu_millis: 8000, mem_bytes: 8 << 30, disk_budget_bytes: 100 << 30, hard: true, gpus: 0 },
        free: frames::Free { cpu_millis: 8000, mem_bytes: 8 << 30, disk_free_bytes: 500 << 30, gpus: 0 },
        slots: vec![],
        running: 0,
        parallel_max: 1,
        refusal: None,
        labels: [("os".to_string(), "linux".to_string())].into(),
        tools: BTreeMap::new(),
        protocol: 6,
        boot_id: "b".into(),
        ledger_id: "l".into(),
        node_version: version.into(),
        features: features.iter().map(|f| f.to_string()).collect(),
        host: None,
        pkg: None,
        sim: None,
    }
}

fn wait_get(c: &mut crate::compute_ws::TestConn) -> frames::ArtifactGet {
    let t0 = Instant::now();
    loop {
        if let Some(g) = c.frames().into_iter().find_map(|f| match f {
            Frame::ArtifactGet(g) => Some(g),
            _ => None,
        }) {
            return g;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "no artifact_get within 10 s");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A prune while a fetch pulls the file leaves the attempt's folder
/// alone, and the fetch finishes; once the fetch is done (and its grace
/// passed) the same folder goes.
#[test]
fn a_prune_during_a_fetch_leaves_it_alone_and_the_fetch_finishes() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    let j = {
        let conn = db.lock();
        node(&conn, "n-fetch-prune");
        job(&conn, "fe7c0000000000000000000000000000", "n-fetch-prune", Some(30 * DAY), false)
    };
    let body: Vec<u8> = (0..3000u32).map(|i| (i % 251) as u8).collect();
    let info = ArtifactInfo { name: "dist/app.tar.gz".into(), sha256: crypto::sha256_hex(&body), bytes: body.len() as u64 };
    let mut conn = crate::compute_ws::test_attach("n-fetch-prune", offer(&[results::FEATURE_ARTIFACTS], "0.46.16"));
    let (job2, inf) = (j.clone(), info.clone());
    let pull = std::thread::spawn(move || crate::compute_ws::fetch_artifact(&job2, &inf, false, Duration::from_secs(30)));
    let get = wait_get(&mut conn);
    let chunk = |seq: u32, data: &[u8], last: bool| {
        Frame::ArtifactChunk(ArtifactChunk { request_id: get.request_id.clone(), seq, data: crypto::b64(data), last, total_bytes: info.bytes, sha256: info.sha256.clone() })
    };
    crate::compute_ws::test_frame("n-fetch-prune", chunk(0, &body[..1000], false));
    let dir = artifacts_dir(&j.id, j.generation);
    // Wait for the first bytes to land in the folder.
    let t0 = Instant::now();
    while rt::measure(&dir).0 < 1000 {
        assert!(t0.elapsed() < Duration::from_secs(10), "the first chunk never reached the folder");
        std::thread::sleep(Duration::from_millis(10));
    }
    // Everything is eligible (--older-than 0): only "in use" can keep it.
    let o = prune(&opts(Some(0), false)).expect("prune during the fetch");
    assert!(o.removed.iter().all(|(u, _)| u.job.as_deref() != Some(j.id.as_str())), "{:?}", ids(&o.removed));
    assert!(o.kept.iter().any(|(u, k)| u.job.as_deref() == Some(j.id.as_str()) && *k == Keep::InUse));
    assert!(dir.exists(), "the folder a fetch is writing stays");
    crate::compute_ws::test_frame("n-fetch-prune", chunk(1, &body[1000..], true));
    let path = pull.join().expect("pull thread").unwrap_or_else(|(c, h)| panic!("the fetch broke: {c}: {h}"));
    assert_eq!(std::fs::read(&path).expect("the fetched file"), body);
    // Just fetched: in use for the grace (the CLI reads it in slices).
    let o = prune(&Opts::now(Some(0), false)).expect("prune inside the grace");
    assert!(o.kept.iter().any(|(u, k)| u.job.as_deref() == Some(j.id.as_str()) && *k == Keep::InUse), "inside the grace after a fetch");
    assert!(path.exists());
    // A slice being read holds it too (the route's guard).
    {
        let _reading = use_artifacts(&j.id, j.generation);
        let o = prune(&opts(Some(0), false)).expect("prune while a slice is read");
        assert!(o.removed.is_empty(), "{:?}", ids(&o.removed));
    }
    let o = prune(&opts(Some(0), false)).expect("prune after");
    assert_eq!(ids(&o.removed), vec![(j.id.clone(), Why::Age)]);
    assert!(!dir.exists());
    crate::compute_ws::test_detach("n-fetch-prune");
}

#[test]
fn only_the_owner_and_admins_prune_pin_or_set_the_rule() {
    use k2_core::connect_users::Role;
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let agent = Who::Agent { project_id: "p".into(), session_id: "s".into() };
    let shell = Who::Shell { project_id: "p".into(), session_id: "s".into() };
    let member = Who::Login { username: "m".into(), role: Role::Member };
    for p in ROUTES {
        for who in [&agent, &shell, &member] {
            let e = handle(p, who, br#"{"job":"x","dryRun":true}"#).expect_err("refused");
            assert_eq!((e.code, e.status), (codes::OWNER_ONLY, 403), "{p} as {who:?}: {}", e.hint);
        }
        let e = handle(p, &Who::Invalid, b"{}").expect_err("refused");
        assert_eq!(e.status, 403, "{p} without a credential");
    }
    let admin = Who::Login { username: "a".into(), role: Role::Admin };
    let v = handle(PRUNE, &admin, br#"{"dryRun":true}"#).expect("an Admin may prune");
    assert_eq!(v["dryRun"], true);
    let v = handle(PRUNE, &Who::Owner, br#"{"dryRun":true,"olderThan":"3d"}"#).expect("the owner may prune");
    assert_eq!(v["maxAgeSecs"], 3 * DAY);
    let e = handle(PRUNE, &Who::Owner, br#"{"olderThan":"soon"}"#).expect_err("a bad duration");
    assert_eq!((e.code, e.status), (codes::BAD_REQUEST, 400));
}

#[test]
fn the_mutating_routes_are_post_only_and_admin_floored() {
    for p in ROUTES {
        let r = crate::routes::route_policy::lookup(p).unwrap_or_else(|| panic!("{p} has no route_policy row"));
        let post_only = POST_ROUTES.contains(p);
        assert!(crate::compute_routes::is_route(p), "{p} on the compute router");
        assert_eq!(crate::compute_routes::is_post_route(p), post_only, "{p}: the dispatcher's 405 guard");
        assert_eq!(r.get.is_none(), post_only, "{p}: a mutating route admits no GET");
        assert!(!crate::session_token::is_agent_verb(p), "{p} is not an agent verb");
    }
    assert!(POST_ROUTES.contains(&PRUNE) && POST_ROUTES.contains(&PIN) && POST_ROUTES.contains(&UNPIN) && POST_ROUTES.contains(&RETENTION_SET));
}

#[test]
fn pin_resolves_a_short_id_and_says_how_many_files_are_here() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    let j = {
        let conn = db.lock();
        node(&conn, "n-pin2");
        job(&conn, "c1c00000000000000000000000000000", "n-pin2", Some(DAY), false)
    };
    let v = handle(PIN, &Who::Owner, br#"{"job":"c1c00000"}"#).expect("pin");
    assert_eq!((v["job"].as_str(), v["pinned"].as_bool(), v["changed"].as_bool()), (Some(j.id.as_str()), Some(true), Some(true)));
    assert!(k2_core::db::read(rt::pins).expect("pins").contains_key(&j.id));
    let v = handle(PIN, &Who::Owner, br#"{"job":"c1c00000"}"#).expect("pin again");
    assert_eq!(v["changed"], false);
    let v = handle(UNPIN, &Who::Owner, br#"{"job":"c1c00000"}"#).expect("unpin");
    assert_eq!((v["pinned"].as_bool(), v["changed"].as_bool()), (Some(false), Some(true)));
    let e = handle(PIN, &Who::Owner, br#"{"job":"nosuchjob"}"#).expect_err("no such job");
    assert_eq!(e.status, 404);
}

#[test]
fn retention_set_changes_the_rule_and_refuses_a_bad_one() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let v = handle(RETENTION_SET, &Who::Owner, br#"{"maxAge":"3d","cap":"5G"}"#).expect("set");
    assert_eq!((v["policy"]["maxAgeSecs"].as_i64(), v["policy"]["capBytes"].as_u64()), (Some(3 * DAY), Some(5 << 30)));
    let e = handle(RETENTION_SET, &Who::Owner, br#"{"cap":"100M"}"#).expect_err("under 1 GB");
    assert_eq!(e.status, 400);
    let e = handle(RETENTION_SET, &Who::Owner, b"{}").expect_err("nothing named");
    assert_eq!(e.status, 400);
    let v = handle(RETENTION_SET, &Who::Owner, br#"{"maxAge":"default","cap":"auto"}"#).expect("back to the defaults");
    assert_eq!((v["policy"]["maxAgeFrom"].as_str(), v["policy"]["capFrom"].as_str()), (Some("default"), Some("auto")));
    let v = handle(RETENTION, &Who::Owner, b"").expect("read");
    assert!(v["line"].as_str().is_some_and(|l| l.contains("keep 7 d after a job ends")), "{v}");
}

#[test]
fn a_retry_of_a_dirty_job_whose_blob_was_pruned_is_refused_clearly() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let db = k2_core::db::shared();
    let j = {
        let conn = db.lock();
        node(&conn, "n-retry");
        job(&conn, "e1e00000000000000000000000000000", "n-retry", Some(DAY), true)
    };
    let blob = put_dirty(&j.id);
    assert!(source_kept(&j).is_ok(), "the blob is here");
    std::fs::remove_file(&blob).expect("rm");
    let e = source_kept(&j).expect_err("blob gone");
    assert_eq!((e.code, e.status), ("source_pruned", 409));
}
