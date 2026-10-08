//! k2-node against a loopback fake controller (§17 Handshake, Enroll,
//! Runner, Ledger, Sync). Temp dirs only; no real network or users.

mod common;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use common::*;
use k2_node::ledger::Ledger;
use k2_node::node::Node;
use k2_node::paths::Layout;
use k2_node_proto::crypto::{self, SigningKey};
use k2_node_proto::frames::{
    self, Blob, Cancel, Direction, Frame, JobState, LogStream, ResumeHint, Revoked, Src, Transfer, TransferKind,
};
use k2_node_proto::pairing::{self, EnrollString};

fn job_done_exit(frames: &[Frame], job: &str) -> (JobState, Option<i32>, Option<String>) {
    let s = states_of(frames, job).into_iter().last().expect("a state for the job");
    (s.state, s.exit.and_then(|e| e.code), s.reason)
}

#[tokio::test(flavor = "multi_thread")]
async fn handshake_good_path_sends_snapshot_then_offer() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "e2e-hs", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    let first = c.next().await;
    match first {
        Frame::JobsSnapshot(s) => assert!(s.last && s.jobs.is_empty(), "{s:?}"),
        other => panic!("first frame must be a jobs snapshot, got {}", other.kind()),
    }
    match c.next().await {
        Frame::Offer(o) => {
            assert_eq!(o.protocol, frames::PROTOCOL);
            assert_eq!(o.labels.get("name").map(|s| s.as_str()), Some("mini-1"));
            assert!(o.tools.contains_key("git"), "{:?}", o.tools);
            assert_eq!(o.refusal, None);
            assert_eq!(o.control, frames::Control::Active);
        }
        other => panic!("then an offer, got {}", other.kind()),
    }
    let st = k2_node::status::read(&n.layout.status()).unwrap();
    assert_eq!(st.state, "online");
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_controller_pin_is_refused() {
    let fake = Fake::start().await;
    let node_key = SigningKey::generate().unwrap();
    fake.register_active(&node_key, "node-1", "mini-1");
    let impostor = SigningKey::generate().unwrap();
    let mut pin = fake.pin_for("node-1", "mini-1");
    pin.controller_fp = impostor.fingerprint();
    pin.controller_public_key_pem = impostor.spki_pem();
    let mut ws = k2_node::session::connect(&fake.base, k2_node::session::ATTACH_PATH).await.unwrap();
    let err = match k2_node::session::attach(&mut ws, &node_key, &pin, "b", "l").await {
        Err(e) => e,
        Ok(_) => panic!("a controller that isn't the pinned one must be refused"),
    };
    assert!(err.contains("fingerprint"), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn enroll_happy_path_and_sas_matches_the_controller() {
    let fake = Fake::start().await;
    let code = pairing::new_code().unwrap();
    let norm = pairing::normalize_code(&code).unwrap();
    fake.codes.lock().unwrap().push(norm.clone());
    let es = EnrollString::new(&code, &fake.key.fingerprint()).unwrap();
    let key = SigningKey::generate().unwrap();
    let mut ws = k2_node::session::connect(&fake.base, k2_node::session::ENROLL_PATH).await.unwrap();
    let done = k2_node::enroll::enroll_on(&mut ws, &key, &fake.base, &es, "mini-1", &BTreeMap::new(), 5).await.unwrap();
    assert_eq!(Some(done.sas.clone()), fake.last_sas.lock().unwrap().clone(), "both ends show the same code");
    assert_eq!(done.pin.controller_fp, fake.key.fingerprint());
    assert_eq!(done.pin.routes, vec![fake.base.clone()]);
    assert!(!done.pin.confirmed);
    assert_eq!(fake.nodes.lock().unwrap().get(&key.fingerprint()).unwrap().state, NodeState::Pending);
    // The code is single-use.
    let mut ws = k2_node::session::connect(&fake.base, k2_node::session::ENROLL_PATH).await.unwrap();
    let err = k2_node::enroll::enroll_on(&mut ws, &key, &fake.base, &es, "mini-1", &BTreeMap::new(), 5).await.err().unwrap();
    assert!(err.contains("code_invalid"), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn enroll_with_the_wrong_fingerprint_prefix_is_refused() {
    let fake = Fake::start().await;
    let code = pairing::new_code().unwrap();
    fake.codes.lock().unwrap().push(pairing::normalize_code(&code).unwrap());
    let other = SigningKey::generate().unwrap();
    let es = EnrollString::new(&code, &other.fingerprint()).unwrap();
    let key = SigningKey::generate().unwrap();
    let mut ws = k2_node::session::connect(&fake.base, k2_node::session::ENROLL_PATH).await.unwrap();
    let err = k2_node::enroll::enroll_on(&mut ws, &key, &fake.base, &es, "mini-1", &BTreeMap::new(), 5).await.err().unwrap();
    assert!(err.contains("impersonating"), "{err}");
    assert!(fake.nodes.lock().unwrap().is_empty(), "no proof was sent, so nothing was registered");
}

#[tokio::test(flavor = "multi_thread")]
async fn pending_node_waits_then_goes_online_when_confirmed() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "e2e-pending", "", |_| {}).await;
    let fp = n.node.key.fingerprint();
    // Flip to pending before the first attach completes is racy; instead
    // re-enter pending and force a reconnect.
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    fake.nodes.lock().unwrap().get_mut(&fp).unwrap().state = NodeState::Pending;
    c.tx.send(Out::Close).unwrap();
    for _ in 0..100 {
        if k2_node::status::read(&n.layout.status()).unwrap().state == "pending" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(k2_node::status::read(&n.layout.status()).unwrap().state, "pending");
    fake.nodes.lock().unwrap().get_mut(&fp).unwrap().state = NodeState::Active;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn forged_and_replayed_frames_close_the_session() {
    let mut fake = Fake::start().await;
    let _n = start_node(&fake, "e2e-forge", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    // Forged: signed by someone else.
    let other = SigningKey::generate().unwrap();
    let f = Frame::Cancel(Cancel { job_id: "x".into(), generation: 1, reason: "cancelled".into() });
    let forged = frames::seal(&other, Direction::ControllerToNode, "whatever", 1, &f).unwrap();
    c.tx.send(Out::Raw(forged)).unwrap();
    assert!(c.wait_closed(Duration::from_secs(5)).await, "a forged frame closes the socket");
    // Replay: a valid frame sent twice (second has a stale seq).
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    c.send(f.clone());
    tokio::time::sleep(Duration::from_millis(200)).await;
    let replay = c.sent_raw.lock().unwrap().last().cloned().unwrap();
    c.tx.send(Out::Raw(replay)).unwrap();
    assert!(c.wait_closed(Duration::from_secs(5)).await, "a replayed frame closes the socket");
}

#[tokio::test(flavor = "multi_thread")]
async fn assign_runs_and_reports_logs_exit_and_a_verified_receipt() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "e2e-run", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    c.send(Frame::Assign(assign(plan("j1", &["/bin/sh", "-c", "echo hi; echo err >&2; exit 3"]), 1)));
    let fr = run_to_receipt(&mut c, "j1").await;
    let logs = logs_of(&fr, "j1");
    assert_eq!(text(&logs, LogStream::Out), "hi\n");
    assert_eq!(text(&logs, LogStream::Err), "err\n");
    let seqs: Vec<u64> = logs.iter().map(|l| l.0).collect();
    assert_eq!(seqs, (1..=seqs.len() as u64).collect::<Vec<_>>(), "continuous from 1");
    assert_eq!(job_done_exit(&fr, "j1"), (JobState::Done, Some(3), None), "an exit code is a result, not a failure");
    let Frame::Receipt(r) = fr.last().unwrap() else { unreachable!() };
    let receipt = r.receipt.verify(n.node.key.spki_der()).expect("receipt verifies against the node key");
    assert_eq!(receipt.state, JobState::Done);
    assert_eq!(receipt.exit.unwrap().code, Some(3));
    assert_eq!(receipt.controller_fp, fake.key.fingerprint());
    assert_eq!(receipt.argv_sha256, frames::argv_sha256(&["/bin/sh".into(), "-c".into(), "echo hi; echo err >&2; exit 3".into()]));
    assert!(!receipt.log.truncated);
    assert_eq!(receipt.log.bytes, 7);
}

#[tokio::test(flavor = "multi_thread")]
async fn job_env_is_scrubbed_and_home_is_the_job_home() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "e2e-env", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    let mut p = plan("env1", &["/usr/bin/env"]);
    p.env.insert("RUST_LOG".into(), "debug".into());
    c.send(Frame::Assign(assign(p, 1)));
    let fr = run_to_receipt(&mut c, "env1").await;
    let out = text(&logs_of(&fr, "env1"), LogStream::Out);
    let env: BTreeMap<String, String> = out.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
    for v in k2_node_proto::env::PROD_REACH_VARS {
        assert!(!env.contains_key(*v), "{v} leaked into the job");
    }
    let k2: Vec<&String> = env.keys().filter(|k| k.starts_with("K2")).collect();
    assert_eq!(k2, vec!["K2_COMPUTE_JOB"]);
    assert_eq!(env["K2_COMPUTE_JOB"], "env1");
    assert_eq!(env["RUST_LOG"], "debug");
    assert_eq!(env["HOME"], n.layout.job_dir("env1", 1).join("home").display().to_string());
    assert_eq!(env["CI"], "1");
    // A daemon-reaching --env is refused at the node too.
    let mut bad = plan("env2", &["/usr/bin/true"]);
    bad.env.insert("K2_HOOK_TOKEN".into(), "x".into());
    c.send(Frame::Assign(assign(bad, 1)));
    let fr = run_to_receipt(&mut c, "env2").await;
    assert_eq!(job_done_exit(&fr, "env2"), (JobState::Failed, None, Some("env_refused".into())));
}

#[tokio::test(flavor = "multi_thread")]
async fn timeout_kills_the_whole_group_including_a_grandchild() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "e2e-timeout", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    let pidfile = n.dir.join("gc.pid");
    let script = format!("sleep 1000 & echo $! > {}; wait", pidfile.display());
    let mut p = plan("t1", &["/bin/sh", "-c", &script]);
    p.limits.max_secs = 1;
    c.send(Frame::Assign(assign(p, 1)));
    let fr = run_to_receipt(&mut c, "t1").await;
    let (state, _, reason) = job_done_exit(&fr, "t1");
    assert_eq!((state, reason.as_deref()), (JobState::Timeout, Some("timeout")));
    let gc: i32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_ne!(unsafe { libc::kill(gc, 0) }, 0, "grandchild {gc} outlived the job");
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_waits_the_grace_then_kills() {
    let mut fake = Fake::start().await;
    let _n = start_node(&fake, "e2e-cancel", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    c.send(Frame::Assign(assign(plan("c1", &["/bin/sh", "-c", "trap '' TERM; echo up; sleep 1000"]), 1)));
    c.until(|f| matches!(f, Frame::Log(l) if l.job_id == "c1")).await;
    let t0 = Instant::now();
    c.send(Frame::Cancel(Cancel { job_id: "c1".into(), generation: 1, reason: "cancelled".into() }));
    let fr = run_to_receipt(&mut c, "c1").await;
    let took = t0.elapsed();
    assert_eq!(job_done_exit(&fr, "c1").0, JobState::Cancelled);
    assert!(took >= Duration::from_millis(450), "SIGTERM was ignored, so the 500 ms grace ran first ({took:?})");
    assert!(took < Duration::from_secs(5), "{took:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn log_sequence_is_continuous_across_a_forced_reconnect() {
    let mut fake = Fake::start().await;
    let _n = start_node(&fake, "e2e-reconnect", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    c.send(Frame::Assign(assign(plan("r1", &["/bin/sh", "-c", "for i in 1 2 3 4 5 6; do echo line$i; sleep 0.25; done"]), 1)));
    let fr = c.until(|f| matches!(f, Frame::Log(l) if l.job_id == "r1")).await;
    let mut got = logs_of(&fr, "r1");
    let last = got.last().unwrap().0;
    fake.resume.lock().unwrap().push(ResumeHint { job_id: "r1".into(), generation: 1, since_seq: last });
    c.tx.send(Out::Close).unwrap();
    let mut c = fake.next_conn().await;
    let fr = run_to_receipt(&mut c, "r1").await;
    got.extend(logs_of(&fr, "r1"));
    let seqs: Vec<u64> = got.iter().map(|l| l.0).collect();
    assert_eq!(seqs, (1..=seqs.len() as u64).collect::<Vec<_>>(), "no gap, no duplicate across the reconnect");
    assert_eq!(text(&got, LogStream::Out), "line1\nline2\nline3\nline4\nline5\nline6\n");
    assert_eq!(job_done_exit(&fr, "r1").0, JobState::Done);
}

#[tokio::test(flavor = "multi_thread")]
async fn log_cap_truncates_with_a_marker_and_the_receipt_says_so() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "e2e-cap", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    let mut p = plan("cap1", &["/bin/sh", "-c", "echo 0123456789abcdefghij"]);
    p.limits.log_cap_bytes = 10;
    c.send(Frame::Assign(assign(p, 1)));
    let fr = run_to_receipt(&mut c, "cap1").await;
    let logs = logs_of(&fr, "cap1");
    assert_eq!(text(&logs, LogStream::Out), "0123456789");
    let marker = text(&logs, LogStream::Sys);
    assert!(marker.contains("log truncated at 10 bytes; 11 bytes dropped"), "{marker}");
    let Frame::Receipt(r) = fr.last().unwrap() else { unreachable!() };
    assert!(r.receipt.verify(n.node.key.spki_der()).unwrap().log.truncated);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replayed_or_stale_generation_never_runs_again() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "e2e-gen", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    let marker = n.dir.join("runs");
    let script = format!("echo x >> {}", marker.display());
    c.send(Frame::Assign(assign(plan("g1", &["/bin/sh", "-c", &script]), 2)));
    run_to_receipt(&mut c, "g1").await;
    // The same fence again, and an older generation: both only report.
    c.send(Frame::Assign(assign(plan("g1", &["/bin/sh", "-c", &script]), 2)));
    let s = c.until(|f| matches!(f, Frame::State(s) if s.job_id == "g1")).await;
    let Frame::State(s) = s.last().unwrap() else { unreachable!() };
    assert_eq!((s.generation, s.state), (2, JobState::Done));
    c.send(Frame::Assign(assign(plan("g1", &["/bin/sh", "-c", &script]), 1)));
    let s = c.until(|f| matches!(f, Frame::State(s) if s.job_id == "g1")).await;
    let Frame::State(s) = s.last().unwrap() else { unreachable!() };
    assert_eq!((s.generation, s.state), (2, JobState::Done), "the newer attempt's state, nothing rerun");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "x\n", "ran exactly once");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reboot_marks_open_jobs_interrupted_and_the_snapshot_says_so() {
    let mut fake = Fake::start().await;
    let dir = k2_node::util::temp_dir("e2e-reboot");
    let layout = Layout::new(dir.join("home"), dir.join("config"));
    layout.ensure().unwrap();
    write_policy(&layout, "");
    struct Boot(String);
    impl k2_node::ledger::Clock for Boot {
        fn now(&self) -> i64 {
            k2_node::util::now()
        }
        fn boot_id(&self) -> String {
            self.0.clone()
        }
    }
    {
        let l = Ledger::open(&layout.ledger()).unwrap();
        l.insert(&assign(plan("old", &["/bin/true"]), 1), &Boot("boot-1".into())).unwrap();
        l.set_running("old", 1, 999_999, None, 1).unwrap();
    }
    let key = k2_node::identity::load_or_create_key(&layout.key()).unwrap();
    fake.register_active(&key, "node-1", "mini-1");
    k2_node::identity::write_pin(&layout.pin(), &fake.pin_for("node-1", "mini-1")).unwrap();
    let mut o = options(layout.clone());
    o.clock = std::sync::Arc::new(Boot("boot-2".into()));
    let node = Node::open(o).await.unwrap();
    let r = node.ledger.lock().unwrap().get("old", 1).unwrap().unwrap();
    assert_eq!((r.state, r.reason.as_deref()), (JobState::Interrupted, Some("node_reboot")));
    let runner = tokio::spawn(k2_node::session::run_forever(node.clone()));
    let mut c = fake.next_conn().await;
    let Frame::JobsSnapshot(s) = c.next().await else { panic!("snapshot first") };
    assert_eq!(s.jobs.len(), 1);
    assert_eq!((s.jobs[0].state, s.jobs[0].reason.as_deref()), (JobState::Interrupted, Some("node_reboot")));
    runner.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paused_node_returns_assigns_to_the_queue() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "e2e-paused", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    k2_node::config::write_control(&n.layout.control(), frames::Control::Paused, "rosson", 5).unwrap();
    // The offer flips to paused within a poll.
    c.until(|f| matches!(f, Frame::Offer(o) if o.control == frames::Control::Paused)).await;
    c.send(Frame::Assign(assign(plan("p1", &["/bin/true"]), 1)));
    let fr = c.until(|f| matches!(f, Frame::State(s) if s.job_id == "p1")).await;
    let s = states_of(&fr, "p1").pop().unwrap();
    assert_eq!((s.state, s.reason.as_deref()), (JobState::Queued, Some("local_paused")));
    assert!(n.node.ledger.lock().unwrap().get("p1", 1).unwrap().is_none(), "a refused assign is never recorded");
    // Stopped interrupts running work.
    k2_node::config::write_control(&n.layout.control(), frames::Control::Active, "rosson", 6).unwrap();
    c.until(|f| matches!(f, Frame::Offer(o) if o.control == frames::Control::Active)).await;
    c.send(Frame::Assign(assign(plan("p2", &["/bin/sh", "-c", "echo up; sleep 1000"]), 1)));
    c.until(|f| matches!(f, Frame::Log(l) if l.job_id == "p2")).await;
    k2_node::config::write_control(&n.layout.control(), frames::Control::Stopped, "rosson", 7).unwrap();
    let fr = run_to_receipt(&mut c, "p2").await;
    let s = states_of(&fr, "p2").pop().unwrap();
    assert_eq!((s.state, s.reason.as_deref()), (JobState::Interrupted, Some("local_stopped")));
}

#[tokio::test(flavor = "multi_thread")]
async fn young_foreign_lock_holds_old_one_does_not_and_smoke_lock_comes_and_goes() {
    let mut fake = Fake::start().await;
    let dir = k2_node::util::temp_dir("e2e-locks-files");
    let foreign = dir.join("sew.lock");
    let smoke = dir.join("k2-smoke.lock");
    std::fs::write(&foreign, "sew build\n").unwrap();
    let extra = format!(
        "foreign_locks = [\"{}\"]\nwrite_smoke_lock = \"{}\"\n",
        foreign.display(),
        smoke.display()
    );
    // write_policy puts foreign_locks/write_smoke_lock first; ours must win,
    // so write the whole file here.
    let n = start_node(&fake, "e2e-locks", "", |_| {}).await;
    std::fs::write(n.layout.policy(), format!("availability = \"always\"\ndisk_floor_gb = 0\n{extra}")).unwrap();
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    c.until(|f| matches!(f, Frame::Offer(o) if o.refusal.as_ref().is_some_and(|r| r.code == "foreign_lock"))).await;
    c.send(Frame::Assign(assign(plan("l1", &["/bin/true"]), 1)));
    let fr = c.until(|f| matches!(f, Frame::State(s) if s.job_id == "l1")).await;
    assert_eq!(states_of(&fr, "l1").pop().unwrap().reason.as_deref(), Some("foreign_lock"));
    // Age the lock past 12 h: reported, not deleted, no longer holding.
    let old = k2_node::util::now() - 13 * 3600;
    let cpath = std::ffi::CString::new(foreign.to_str().unwrap()).unwrap();
    let tv = [libc::timeval { tv_sec: old as _, tv_usec: 0 }, libc::timeval { tv_sec: old as _, tv_usec: 0 }];
    assert_eq!(unsafe { libc::utimes(cpath.as_ptr(), tv.as_ptr()) }, 0);
    c.until(|f| matches!(f, Frame::Offer(o) if o.refusal.is_none())).await;
    assert!(foreign.exists());
    let mut stale = vec![];
    for _ in 0..40 {
        stale = k2_node::status::read(&n.layout.status()).unwrap().stale_locks;
        if !stale.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(stale, vec![foreign.display().to_string()], "status reports the old lock");
    c.send(Frame::Assign(assign(plan("l2", &["/bin/sh", "-c", "echo up; sleep 0.5"]), 1)));
    c.until(|f| matches!(f, Frame::Log(l) if l.job_id == "l2")).await;
    let body = std::fs::read_to_string(&smoke).expect("smoke lock written while the job runs");
    assert!(body.contains("job=l2"), "{body}");
    run_to_receipt(&mut c, "l2").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!smoke.exists(), "our smoke lock is removed after the last job");
}

#[tokio::test(flavor = "multi_thread")]
async fn revoked_cancels_jobs_and_the_node_stops_dialing() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "e2e-revoke", "", |_| {}).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    c.send(Frame::Assign(assign(plan("v1", &["/bin/sh", "-c", "echo up; sleep 1000"]), 1)));
    c.until(|f| matches!(f, Frame::Log(l) if l.job_id == "v1")).await;
    let before = fake.attaches.load(std::sync::atomic::Ordering::SeqCst);
    c.send(Frame::Revoked(Revoked { reason: "removed by rosson".into() }));
    assert!(c.wait_closed(Duration::from_secs(5)).await);
    for _ in 0..100 {
        if n.node.ledger.lock().unwrap().get("v1", 1).unwrap().unwrap().state.is_terminal() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let r = n.node.ledger.lock().unwrap().get("v1", 1).unwrap().unwrap();
    assert_eq!((r.state, r.reason.as_deref()), (JobState::Cancelled, Some("node_removed")));
    let pin = k2_node::identity::read_pin(&n.layout.pin()).unwrap().unwrap();
    assert!(pin.revoked);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(fake.attaches.load(std::sync::atomic::Ordering::SeqCst), before, "no redial after revoke");
    assert_eq!(k2_node::status::read(&n.layout.status()).unwrap().state, "revoked");
}

/// The controller's side of a bundle: temp ref + `git bundle create`.
fn bundle_for(repo: &std::path::Path, job: &str, sha: &str, out: &std::path::Path) -> Vec<u8> {
    let g = |args: &[&str]| {
        let o = std::process::Command::new("git").args(args).current_dir(repo).env("GIT_CONFIG_GLOBAL", "/dev/null").output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
    };
    let r = format!("refs/k2-compute/{job}");
    g(&["update-ref", &r, sha]);
    g(&["bundle", "create", out.to_str().unwrap(), &r]);
    g(&["update-ref", "-d", &r]);
    std::fs::read(out).unwrap()
}

fn send_blob(c: &Conn, job: &str, kind: TransferKind, data: &[u8], chunk: usize) {
    let sha = crypto::sha256_hex(data);
    let chunks: Vec<&[u8]> = if data.is_empty() { vec![&[]] } else { data.chunks(chunk).collect() };
    let n = chunks.len();
    for (i, ch) in chunks.into_iter().enumerate() {
        c.send(Frame::Transfer(Transfer {
            job_id: job.into(),
            generation: 1,
            kind,
            seq: i as u32,
            data: crypto::b64(ch),
            last: i + 1 == n,
            total_bytes: data.len() as u64,
            sha256: sha.clone(),
        }));
    }
}

fn git_out(repo: &std::path::Path, args: &[&str]) -> String {
    let o = std::process::Command::new("git").args(args).current_dir(repo).env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@e").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@e")
        .output().unwrap();
    assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn temp_repo() -> (std::path::PathBuf, String, String) {
    let d = k2_node::util::temp_dir("e2e-repo").join("repo");
    std::fs::create_dir_all(&d).unwrap();
    git_out(&d, &["init", "-q", "-b", "main"]);
    std::fs::write(d.join("a.txt"), "one\n").unwrap();
    git_out(&d, &["add", "."]);
    git_out(&d, &["commit", "-q", "-m", "one"]);
    let first = git_out(&d, &["rev-parse", "HEAD"]);
    std::fs::write(d.join("a.txt"), "two\n").unwrap();
    std::fs::write(d.join("bin.dat"), [0u8, 200, 1]).unwrap();
    git_out(&d, &["add", "."]);
    git_out(&d, &["commit", "-q", "-m", "two"]);
    let second = git_out(&d, &["rev-parse", "HEAD"]);
    (d, first, second)
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_by_bundle_runs_in_a_worktree_at_the_commit_in_a_warm_slot_next_time() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "e2e-sync", "", |_| {}).await;
    let (repo, first, second) = temp_repo();
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    let mut p = plan("s1", &["/bin/sh", "-c", "cat a.txt; pwd; echo $CARGO_TARGET_DIR"]);
    p.src = Some(Src { project_key: "pk1".into(), remote_url: None, commit: second.clone(), dirty: None, slots: 1 });
    c.send(Frame::Assign(assign(p, 1)));
    let fr = c.until(|f| matches!(f, Frame::Need(_))).await;
    let Frame::Need(need) = fr.last().unwrap() else { unreachable!() };
    assert_eq!((need.kind, need.want.as_str()), (TransferKind::Bundle, second.as_str()));
    assert!(need.have.is_empty(), "a fresh mirror has nothing");
    let bundle = bundle_for(&repo, "s1", &second, &n.dir.join("b1.bundle"));
    send_blob(&c, "s1", TransferKind::Bundle, &bundle, 1000);
    let fr = run_to_receipt(&mut c, "s1").await;
    let out = text(&logs_of(&fr, "s1"), LogStream::Out);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "two");
    assert_eq!(lines[1], n.layout.job_dir("s1", 1).join("src").display().to_string());
    assert_eq!(lines[2], n.layout.project_cache("pk1").join("target-1").display().to_string());
    let running = states_of(&fr, "s1").into_iter().find(|s| s.src.is_some()).expect("a state with src");
    let src = running.src.unwrap();
    assert_eq!(src.commit, second);
    assert_eq!(src.tree, git_out(&repo, &["rev-parse", &format!("{second}^{{tree}}")]));
    assert_eq!((src.slot, src.warmth.as_deref()), (Some(1), Some("cold")));
    // Second job at the older commit: already in the mirror (no Need), warm slot.
    let mut p = plan("s2", &["/bin/sh", "-c", "cat a.txt"]);
    p.src = Some(Src { project_key: "pk1".into(), remote_url: None, commit: first.clone(), dirty: None, slots: 1 });
    c.send(Frame::Assign(assign(p, 1)));
    let fr = run_to_receipt(&mut c, "s2").await;
    assert!(!fr.iter().any(|f| matches!(f, Frame::Need(_))), "history is already on the node");
    assert_eq!(text(&logs_of(&fr, "s2"), LogStream::Out), "one\n");
    let src = states_of(&fr, "s2").into_iter().rev().find_map(|s| s.src).unwrap();
    assert_eq!(src.warmth.as_deref(), Some("warm"));
}

#[tokio::test(flavor = "multi_thread")]
async fn dirty_blob_is_applied_and_a_bad_digest_fails_the_job() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "e2e-dirty", "", |_| {}).await;
    let (repo, _first, second) = temp_repo();
    // Dirty working tree: a modified text file, a modified binary, an untracked file.
    std::fs::write(repo.join("a.txt"), "dirty\n").unwrap();
    std::fs::write(repo.join("bin.dat"), [7u8, 0, 255]).unwrap();
    let stage = n.dir.join("stage");
    std::fs::create_dir_all(stage.join("k2-dirty/untracked/sub")).unwrap();
    let diff = std::process::Command::new("git").args(["diff", "--binary", "HEAD"]).current_dir(&repo).output().unwrap();
    std::fs::write(stage.join("k2-dirty/patch.diff"), diff.stdout).unwrap();
    std::fs::write(stage.join("k2-dirty/untracked/sub/new.txt"), "new\n").unwrap();
    let tar = n.dir.join("dirty.tar");
    assert!(std::process::Command::new("tar").args(["-cf", tar.to_str().unwrap(), "-C", stage.to_str().unwrap(), "k2-dirty"]).status().unwrap().success());
    let blob = std::fs::read(&tar).unwrap();
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    let mut p = plan("d1", &["/bin/sh", "-c", "cat a.txt sub/new.txt; od -An -tu1 bin.dat"]);
    p.src = Some(Src {
        project_key: "pk2".into(),
        remote_url: Some(repo.display().to_string()),
        commit: second.clone(),
        dirty: Some(Blob { sha256: crypto::sha256_hex(&blob), bytes: blob.len() as u64 }),
        slots: 1,
    });
    c.send(Frame::Assign(assign(p, 1)));
    let fr = c.until(|f| matches!(f, Frame::Need(_))).await;
    let Frame::Need(need) = fr.last().unwrap() else { unreachable!() };
    assert_eq!(need.kind, TransferKind::Dirty, "history came from the reachable remote, so only the dirty blob is asked for");
    send_blob(&c, "d1", TransferKind::Dirty, &blob, 4096);
    let fr = run_to_receipt(&mut c, "d1").await;
    let out = text(&logs_of(&fr, "d1"), LogStream::Out);
    assert!(out.starts_with("dirty\nnew\n"), "{out}");
    assert_eq!(out.lines().nth(2).unwrap().split_whitespace().collect::<Vec<_>>(), vec!["7", "0", "255"]);
    let src = states_of(&fr, "d1").into_iter().rev().find_map(|s| s.src).unwrap();
    assert_eq!(src.dirty_sha256.as_deref(), Some(crypto::sha256_hex(&blob).as_str()));
    // A blob that doesn't match the plan's digest fails the job.
    let mut p = plan("d2", &["/bin/true"]);
    p.src = Some(Src {
        project_key: "pk2".into(),
        remote_url: None,
        commit: second.clone(),
        dirty: Some(Blob { sha256: "0".repeat(64), bytes: blob.len() as u64 }),
        slots: 1,
    });
    c.send(Frame::Assign(assign(p, 1)));
    c.until(|f| matches!(f, Frame::Need(_))).await;
    send_blob(&c, "d2", TransferKind::Dirty, &blob, 4096);
    let fr = run_to_receipt(&mut c, "d2").await;
    let s = states_of(&fr, "d2").pop().unwrap();
    assert_eq!((s.state, s.reason.as_deref()), (JobState::Failed, Some("sync_failed")));
}

#[tokio::test(flavor = "multi_thread")]
async fn rss_over_the_cap_kills_the_job() {
    struct Huge;
    impl k2_node::runner::RssSampler for Huge {
        fn group_rss(&self, _pgid: i32) -> Option<u64> {
            Some(1 << 50)
        }
    }
    let mut fake = Fake::start().await;
    let _n = start_node(&fake, "e2e-rss", "", |o| {
        o.rss = std::sync::Arc::new(Huge);
        o.rss_every = Duration::from_millis(100);
    })
    .await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    c.send(Frame::Assign(assign(plan("m1", &["/bin/sleep", "30"]), 1)));
    let fr = run_to_receipt(&mut c, "m1").await;
    let s = states_of(&fr, "m1").pop().unwrap();
    assert_eq!((s.state, s.reason.as_deref()), (JobState::Failed, Some("mem_cap")));
}
