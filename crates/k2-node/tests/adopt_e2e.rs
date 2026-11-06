//! Jobs keep running through k2-node updates (prd-compute-update-continuity-v1
//! U2–U4): a node process detaches, a new one adopts.
//!
//! Each node "process" runs in its own tokio runtime; shutting the runtime
//! down is the process exiting (every task of it is gone, as after a real
//! exit). Jobs are held by the real `k2-node job-hold` binary, as on macOS
//! nodes (here forced on with `NodeOptions::job_holder`, so the same tests
//! run on Linux without root; the Linux wall's units need root and systemd:
//! see `job_wall_linux.rs`). The fake controller lives in a third runtime
//! and sees both sessions.
// pending k2-node Windows port: k2-node (the compute node runtime) is Unix-only
// until its Windows port (Phase 3: Job Objects instead of uids and cgroups).
#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::*;
use k2_node_proto::frames::{Frame, JobState, LogStream, StateUpdate};
use k2_node_proto::share::{ServersFile, ShareLimits, ShareState, Trust};
use tokio::runtime::Runtime;

fn rt() -> Runtime {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap()
}

fn holder() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_k2-node"))
}

/// One k2-node process: its own runtime.
struct NodeProc {
    rt: Option<Runtime>,
    n: Option<TestNode>,
}

impl NodeProc {
    fn start(fake: &Fake, dir: &Path) -> Self {
        let rt = rt();
        let n = rt.block_on(start_node_in(fake, dir.to_path_buf(), "", |o| {
            o.job_holder = Some(holder());
            o.tenant_adopt_wait = Duration::from_secs(5);
        }));
        Self { rt: Some(rt), n: Some(n) }
    }

    fn node(&self) -> &TestNode {
        self.n.as_ref().unwrap()
    }

    /// The executor's `keep` restart: this process detaches (§5.4).
    fn detach(&self) -> (usize, usize) {
        let node = self.node().node.clone();
        self.rt.as_ref().unwrap().block_on(async move { node.detach_for_update().await })
    }

    /// The process exits: every task of it ends.
    fn exit(mut self) {
        drop(self.n.take());
        self.rt.take().unwrap().shutdown_timeout(Duration::from_secs(5));
    }
}

/// Prints `out-1..out-N` and `err-1..err-N` over a few seconds, records
/// its pid, writes an artifact, exits 7.
fn script(n: u32, gap: &str) -> String {
    format!("echo $$ > pid; i=0; while [ $i -lt {n} ]; do i=$((i+1)); echo out-$i; echo err-$i >&2; sleep {gap}; done; echo made > art.txt; exit 7")
}

fn want(stream: &str, n: u32) -> String {
    (1..=n).map(|i| format!("{stream}-{i}\n")).collect()
}

fn job_plan(id: &str, n: u32, gap: &str) -> k2_node_proto::frames::Plan {
    let mut p = plan(id, &["/bin/sh", "-c", &script(n, gap)]);
    p.artifacts = vec!["art.txt".into()];
    p
}

fn job_pid(dir: &Path) -> i32 {
    std::fs::read_to_string(dir.join("pid")).expect("the job wrote its pid").trim().parse().expect("a pid")
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks the pid.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn wait_dead(pid: i32, within: Duration) {
    let t0 = std::time::Instant::now();
    while alive(pid) {
        assert!(t0.elapsed() < within, "process {pid} still runs after {within:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn last_state(fr: &[Frame], job: &str) -> StateUpdate {
    states_of(fr, job).into_iter().last().expect("a state for the job")
}

/// Wait on `c` until `job`'s stdout shows `line`.
fn wait_line(main: &Runtime, c: &mut Conn, job: &str, line: &str) {
    let needle = format!("{line}\n");
    let mut seen = String::new();
    main.block_on(async {
        c.until(|f| {
            if let Frame::Log(l) = f {
                if l.job_id == job && l.stream == LogStream::Out {
                    seen.push_str(&String::from_utf8_lossy(&k2_node_proto::crypto::unb64(&l.data).unwrap()));
                }
            }
            seen.contains(&needle)
        })
        .await
    });
}

/// What the real controller does on the next attach: it names the jobs it
/// still thinks run (`Welcome.resume`), so their end ships even when it
/// happened before the session (from seq 0: the whole log).
fn resume(fake: &Fake, jobs: &[&str]) {
    *fake.resume.lock().unwrap() = jobs.iter().map(|j| k2_node_proto::frames::ResumeHint { job_id: j.to_string(), generation: 1, since_seq: 0 }).collect();
}

fn ledger_of(layout: &k2_node::paths::Layout, slot: u8) -> k2_node::ledger::Ledger {
    k2_node::ledger::Ledger::open(&layout.tenant(slot).ledger()).unwrap()
}

/// The whole output arrives exactly once, in order, across the two
/// processes' sessions, and the end is the job's own.
fn assert_complete(fr: &[Frame], job: &str, n: u32, layout: &k2_node::paths::Layout) {
    let logs = logs_of(fr, job);
    assert_eq!(text(&logs, LogStream::Out), want("out", n), "stdout: complete, no gap, no repeat");
    assert_eq!(text(&logs, LogStream::Err), want("err", n), "stderr: complete, no gap, no repeat");
    let seqs: Vec<u64> = logs.iter().map(|l| l.0).collect();
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "one journal, no seq gap: {seqs:?}");
    let sys = text(&logs, LogStream::Sys);
    assert!(sys.contains("is restarting for an update; this job keeps running"), "{sys}");
    assert!(sys.contains("adopted by k2-node"), "{sys}");
    let end = last_state(fr, job);
    assert_eq!((end.state, end.exit.as_ref().and_then(|e| e.code), end.reason.as_deref()), (JobState::Done, Some(7), None), "{end:?}");
    let o = end.outputs.expect("outputs");
    assert_eq!(o.artifacts.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), vec!["art.txt"]);
    assert_eq!(o.artifacts[0].sha256, k2_node_proto::crypto::sha256_hex(b"made\n"));
    let Some(Frame::Receipt(r)) = fr.last() else { panic!("no receipt") };
    let key = k2_node::identity::load_or_create_key(&layout.key()).unwrap();
    let receipt = r.receipt.verify(key.spki_der()).expect("the receipt verifies with the node's key");
    assert_eq!((receipt.state, receipt.exit.as_ref().and_then(|e| e.code)), (JobState::Done, Some(7)));
    assert_eq!(receipt.artifacts, o.artifacts, "the signed receipt lists the artifact");
}

/// T9 (holder form) + the solo update: a job keeps running across a node
/// restart. Its output is complete (no gap, no repeat), its exit code is
/// its own, its artifact is collected, and its files are cleaned up. Once
/// with the job ending after the new process is up, once with it ending
/// while no k2-node runs (§5.6 "ended while away").
#[test]
fn a_job_runs_across_a_node_restart_with_its_whole_output_exit_and_artifacts() {
    for away in [false, true] {
        let main = rt();
        let mut fake = main.block_on(Fake::start());
        let dir = k2_node::util::temp_dir(if away { "adopt-away" } else { "adopt-run" });
        let a = NodeProc::start(&fake, &dir);
        let layout = a.node().layout.clone();
        let mut c1 = main.block_on(fake.next_conn());
        main.block_on(ready(&mut c1));
        c1.send(Frame::Assign(assign(job_plan("j1", 12, "0.25"), 1)));
        wait_line(&main, &mut c1, "j1", "out-3");
        assert_eq!(a.detach(), (1, 0), "the running job is kept, nothing stopped");
        a.exit();
        let job_dir = layout.job_dir("j1", 1);
        let pid = job_pid(&job_dir);
        assert!(alive(pid), "the job outlived its k2-node process");
        {
            let l = ledger_of(&layout, 0);
            let row = l.get("j1", 1).unwrap().unwrap();
            assert_eq!(row.state, JobState::Running, "its row stays open for the next process");
            let f: k2_node::node::adopt::RunFacts = serde_json::from_str(row.run_json.as_deref().expect("run facts")).unwrap();
            assert_eq!(f.kind, k2_node::node::adopt::RunKind::Holder);
            assert!(f.out.as_ref().unwrap().off > 0, "the journal's offset is recorded");
        }
        if away {
            wait_dead(pid, Duration::from_secs(20));
        }
        resume(&fake, &["j1"]);
        let b = NodeProc::start(&fake, &dir);
        let mut c2 = main.block_on(fake.next_conn());
        let fr = main.block_on(run_to_receipt(&mut c2, "j1"));
        assert_complete(&fr, "j1", 12, &layout);
        let row = ledger_of(&layout, 0).get("j1", 1).unwrap().unwrap();
        assert_eq!((row.state, row.exit.and_then(|e| e.code)), (JobState::Done, Some(7)));
        let spool: Vec<_> = std::fs::read_dir(layout.state().join("spool")).map(|r| r.flatten().map(|e| e.file_name()).collect()).unwrap_or_default();
        assert!(spool.is_empty(), "the output files go with the job: {spool:?}");
        assert!(!layout.state().join("hold").join("j1-g1").exists(), "the holder folder goes too");
        let st = k2_node::status::read(&layout.status()).unwrap();
        assert!(st.adopted.is_empty(), "nothing adopted runs any more: {:?}", st.adopted);
        b.exit();
    }
}

/// C7: the holder dies while no k2-node runs (the job keeps running in its
/// own group). The next process can't know how it would have ended: it
/// stops what's left of the group and ends the job `interrupted /
/// node_update`, never `done`, and never leaves the job running unwatched.
#[test]
fn a_holder_that_dies_while_the_node_is_away_ends_its_job_and_stops_its_group() {
    let main = rt();
    let mut fake = main.block_on(Fake::start());
    let dir = k2_node::util::temp_dir("adopt-holder-gone");
    let a = NodeProc::start(&fake, &dir);
    let layout = a.node().layout.clone();
    let mut c1 = main.block_on(fake.next_conn());
    main.block_on(ready(&mut c1));
    c1.send(Frame::Assign(assign(job_plan("h1", 200, "0.1"), 1)));
    wait_line(&main, &mut c1, "h1", "out-2");
    assert_eq!(a.detach(), (1, 0));
    a.exit();
    let pid = job_pid(&layout.job_dir("h1", 1));
    let f: k2_node::node::adopt::RunFacts = serde_json::from_str(ledger_of(&layout, 0).get("h1", 1).unwrap().unwrap().run_json.as_deref().unwrap()).unwrap();
    let h = f.holder.expect("holder facts");
    // SAFETY: the holder's pid, just read from its facts.
    assert_eq!(unsafe { libc::kill(h.pid as i32, libc::SIGKILL) }, 0);
    wait_dead(h.pid as i32, Duration::from_secs(5));
    assert!(alive(pid), "the job itself still runs, unwatched");
    resume(&fake, &["h1"]);
    let b = NodeProc::start(&fake, &dir);
    let mut c2 = main.block_on(fake.next_conn());
    let fr = main.block_on(run_to_receipt(&mut c2, "h1"));
    let end = last_state(&fr, "h1");
    assert_eq!((end.state, end.reason.as_deref()), (JobState::Interrupted, Some("node_update")), "{end:?}");
    assert!(end.detail.as_deref().is_some_and(|d| d.contains("holder")), "{:?}", end.detail);
    wait_dead(pid, Duration::from_secs(5));
    b.exit();
}

/// C10/T23: run facts newer than this build knows (a rollback from a newer
/// k2-node) are never adopted: the job is stopped cleanly and ends
/// `interrupted / node_update`.
#[test]
fn run_facts_from_a_newer_build_are_stopped_not_adopted() {
    let main = rt();
    let mut fake = main.block_on(Fake::start());
    let dir = k2_node::util::temp_dir("adopt-newer");
    let a = NodeProc::start(&fake, &dir);
    let layout = a.node().layout.clone();
    let mut c1 = main.block_on(fake.next_conn());
    main.block_on(ready(&mut c1));
    c1.send(Frame::Assign(assign(job_plan("v1", 200, "0.1"), 1)));
    wait_line(&main, &mut c1, "v1", "out-2");
    assert_eq!(a.detach(), (1, 0));
    a.exit();
    let pid = job_pid(&layout.job_dir("v1", 1));
    {
        let l = ledger_of(&layout, 0);
        let mut f: serde_json::Value = serde_json::from_str(l.get("v1", 1).unwrap().unwrap().run_json.as_deref().unwrap()).unwrap();
        f["v"] = serde_json::json!(k2_node::node::adopt::RUN_V + 1);
        l.set_run("v1", 1, &f.to_string()).unwrap();
    }
    resume(&fake, &["v1"]);
    let b = NodeProc::start(&fake, &dir);
    // No executor restarted it here (no mark): a plain restart's reason.
    let row = ledger_of(&layout, 0).get("v1", 1).unwrap().unwrap();
    assert_eq!((row.state, row.reason.as_deref()), (JobState::Interrupted, Some("node_restart")));
    wait_dead(pid, Duration::from_secs(10));
    assert!(b.node().node.running_ids().is_empty(), "nothing adopted");
    let mut c2 = main.block_on(fake.next_conn());
    let fr = main.block_on(c2.until(|f| matches!(f, Frame::State(s) if s.job_id == "v1" && s.state.is_terminal())));
    assert_eq!(last_state(&fr, "v1").reason.as_deref(), Some("node_restart"));
    b.exit();
}

/// C1/T17: once adopted, the disk guard's orphan sweep and a cancel on
/// attach never finish the job behind its back; a cancel reaches the
/// adopted job itself (the holder stops it) and the end is `cancelled`.
#[test]
fn an_adopted_job_is_no_orphan_and_a_cancel_reaches_it() {
    let main = rt();
    let mut fake = main.block_on(Fake::start());
    let dir = k2_node::util::temp_dir("adopt-cancel");
    let a = NodeProc::start(&fake, &dir);
    let layout = a.node().layout.clone();
    let mut c1 = main.block_on(fake.next_conn());
    main.block_on(ready(&mut c1));
    c1.send(Frame::Assign(assign(job_plan("k1", 600, "0.1"), 1)));
    wait_line(&main, &mut c1, "k1", "out-2");
    assert_eq!(a.detach(), (1, 0));
    a.exit();
    let pid = job_pid(&layout.job_dir("k1", 1));
    resume(&fake, &["k1"]);
    let b = NodeProc::start(&fake, &dir);
    let node = b.node().node.clone();
    assert_eq!(node.running_ids(), vec!["k1".to_string()], "adopted into the running map");
    assert_eq!(b.rt.as_ref().unwrap().block_on({
        let n = node.clone();
        async move { n.reap_orphans() }
    }), 0, "the orphan sweep leaves an adopted job alone");
    let mut c2 = main.block_on(fake.next_conn());
    main.block_on(ready(&mut c2));
    assert!(alive(pid), "still running after the sweep");
    c2.send(Frame::Cancel(k2_node_proto::frames::Cancel { job_id: "k1".into(), generation: 1, reason: "cancelled".into() }));
    let fr = main.block_on(run_to_receipt(&mut c2, "k1"));
    let end = last_state(&fr, "k1");
    assert_eq!((end.state, end.reason.as_deref()), (JobState::Cancelled, Some("cancelled")), "{end:?}");
    wait_dead(pid, Duration::from_secs(5));
    b.exit();
}

/// T13 / G5: a shared node. The owner's job and another server's job both
/// keep running through the restart: the owner's is adopted at start, the
/// other server's when its slot opens again (C2). Both end with their
/// whole output on their own server's session.
#[test]
fn on_a_shared_node_both_servers_jobs_keep_running() {
    let main = rt();
    let mut fa = main.block_on(Fake::start());
    let mut fb = main.block_on(Fake::start());
    let dir = k2_node::util::temp_dir("adopt-shared");
    let a = NodeProc::start(&fa, &dir);
    let layout = a.node().layout.clone();
    let key = k2_node::identity::load_or_create_key(&layout.key()).unwrap();
    fb.register_active(&key, "node-b", "mini-1");
    let tl = layout.tenant(1);
    tl.ensure_state().unwrap();
    k2_node::identity::write_pin(&tl.pin(), &fb.pin_for("node-b", "mini-1")).unwrap();
    let limits = ShareLimits { max_running: 4, max_queued: 10, weight: 1, ..ShareLimits::new_server(Trust::Own, false) };
    let mut f = ServersFile::new(&fa.key.fingerprint(), "a.k2.dev", limits, 8, k2_node::util::now(), "local:test");
    let mut e = f.servers[0].clone();
    e.slot = 1;
    e.label = "b.k2.dev".into();
    e.fp_prefix = fb.key.fingerprint()[..16].to_string();
    e.trust = Trust::Own;
    e.state = ShareState::Active;
    f.servers.push(e);
    k2_node::admin::servers::write_file(&layout.config, &f).unwrap();
    let mut ca = main.block_on(fa.next_conn());
    main.block_on(ready(&mut ca));
    let mut cb = main.block_on(fb.next_conn());
    main.block_on(ready(&mut cb));
    let mut pb = job_plan("tb", 16, "0.25");
    pb.node_id = "node-b".into();
    ca.send(Frame::Assign(assign(job_plan("ta", 16, "0.25"), 1)));
    cb.send(Frame::Assign(assign(pb, 1)));
    wait_line(&main, &mut ca, "ta", "out-3");
    wait_line(&main, &mut cb, "tb", "out-3");
    assert_eq!(a.detach(), (2, 0), "both servers' jobs are kept");
    a.exit();
    resume(&fa, &["ta"]);
    resume(&fb, &["tb"]);
    let b = NodeProc::start(&fa, &dir);
    let mut ca2 = main.block_on(fa.next_conn());
    let fra = main.block_on(run_to_receipt(&mut ca2, "ta"));
    assert_complete(&fra, "ta", 16, &layout);
    let mut cb2 = main.block_on(fb.next_conn());
    let frb = main.block_on(run_to_receipt(&mut cb2, "tb"));
    assert_complete(&frb, "tb", 16, &layout);
    let t1: Arc<k2_node::node::Tenant> = b.node().node.tenant(1).expect("slot 1 opened again");
    assert_eq!(t1.ledger.lock().unwrap().get("tb", 1).unwrap().unwrap().state, JobState::Done);
    b.exit();
}

/// C2/T18: another server's job is running when its line goes away while
/// the node is restarting. Its slot never opens again, so the new process
/// stops the job (never leaves it running with nobody watching) before
/// anything of the slot is forgotten.
#[test]
fn a_slot_that_never_opens_again_has_its_jobs_stopped() {
    let main = rt();
    let fa = main.block_on(Fake::start());
    let mut fb = main.block_on(Fake::start());
    let dir = k2_node::util::temp_dir("adopt-slot-gone");
    let a = NodeProc::start(&fa, &dir);
    let layout = a.node().layout.clone();
    let key = k2_node::identity::load_or_create_key(&layout.key()).unwrap();
    fb.register_active(&key, "node-b", "mini-1");
    let tl = layout.tenant(1);
    tl.ensure_state().unwrap();
    k2_node::identity::write_pin(&tl.pin(), &fb.pin_for("node-b", "mini-1")).unwrap();
    let limits = ShareLimits { max_running: 4, max_queued: 10, weight: 1, ..ShareLimits::new_server(Trust::Own, false) };
    let mut f = ServersFile::new(&fa.key.fingerprint(), "a.k2.dev", limits, 8, k2_node::util::now(), "local:test");
    let mut e = f.servers[0].clone();
    e.slot = 1;
    e.label = "b.k2.dev".into();
    e.fp_prefix = fb.key.fingerprint()[..16].to_string();
    e.trust = Trust::Own;
    e.state = ShareState::Active;
    f.servers.push(e);
    k2_node::admin::servers::write_file(&layout.config, &f).unwrap();
    let mut cb = main.block_on(fb.next_conn());
    main.block_on(ready(&mut cb));
    let mut pb = job_plan("gone", 600, "0.1");
    pb.node_id = "node-b".into();
    cb.send(Frame::Assign(assign(pb, 1)));
    wait_line(&main, &mut cb, "gone", "out-2");
    assert_eq!(a.detach(), (1, 0));
    a.exit();
    let pid = job_pid(&layout.tenant(1).job_dir("gone", 1));
    // The owner wipes server B's line while the node is down.
    f.servers.retain(|s| s.slot != 1);
    k2_node::admin::servers::write_file(&layout.config, &f).unwrap();
    let b = NodeProc::start(&fa, &dir);
    // Stopped at start, before the servers loop forgets the slot's state.
    wait_dead(pid, Duration::from_secs(15));
    assert!(!b.node().node.running_ids().contains(&"gone".to_string()), "never adopted");
    b.exit();
}
