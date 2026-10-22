//! The Linux job wall for real (0.46.3, N3): the root executor starts each
//! job as another user in a sandboxed transient systemd unit.
//!
//! Needs root and systemd ≥ 245, so every test is `#[ignore]` and runs on
//! purpose, on a Linux box, as root:
//!
//!   cargo test -p k2-node --test job_wall_linux -- --ignored --test-threads=1
//!
//! Real users: the job user is `nobody`; the node runs in the test process
//! (root) with the group of a real node user, `k2nodet` (made once in the
//! box), so the job runs as `nobody` with `k2nodet`'s group, as a real job
//! runs as `k2job` with `k2node`'s group (never group 0: the executor
//! refuses it). Nothing on the machine is
//! changed: each test makes its own dir under /var/lib (the unit hides
//! /tmp, /home and /root, so a temp dir there would vanish inside it) and
//! removes it; the units are transient. Every wall check runs a control
//! first (the same probe without the wall reaches the thing).

#![cfg(target_os = "linux")]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::*;
use k2_node::admin::exec::{ExecPaths, Executor, SystemPlatform};
use k2_node::admin::server::{self, PeerRule};
use k2_node::wall::Wall;
use k2_node_proto::frames::{Cancel, Frame, JobState, LogStream};

/// One test's world: a node home + config + executor state + binary under
/// /var/lib/k2-nodesec-test-<id>, and the root executor serving a socket.
struct Rig {
    base: PathBuf,
    socket: PathBuf,
    bin: PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// The test node user and its gid; the test process takes that group as
/// its effective gid (see the module doc).
fn node_test_user() -> (String, u32) {
    let name = "k2nodet";
    if server::user_ids(name).is_none() {
        let st = std::process::Command::new("useradd")
            .args(["--system", "--user-group", "--no-create-home", "--home-dir", "/nonexistent", "--shell", "/usr/sbin/nologin", name])
            .status()
            .unwrap();
        assert!(st.success(), "useradd {name}");
    }
    let (_, gid) = server::user_ids(name).unwrap();
    // SAFETY: glibc applies setegid to every thread of the process.
    assert_eq!(unsafe { libc::setegid(gid) }, 0, "setegid {gid}");
    (name.to_string(), gid)
}

fn rig(tag: &str) -> Rig {
    assert_eq!(unsafe { libc::geteuid() }, 0, "the Linux wall tests run as root (cargo test … -- --ignored)");
    let id = k2_node_proto::crypto::random_id().unwrap();
    let base = PathBuf::from(format!("/var/lib/k2-nodesec-test-{tag}-{}", &id[..8]));
    std::fs::create_dir_all(&base).unwrap();
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();
    let config = base.join("config");
    let state = base.join("admin-state");
    for d in [&config, &state, &base.join("home")] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
    // The node home like the installer makes it: 0750, group = the node's.
    let (node_user, node_gid) = node_test_user();
    std::os::unix::fs::chown(base.join("home"), Some(0), Some(node_gid)).unwrap();
    std::fs::set_permissions(base.join("home"), std::fs::Permissions::from_mode(0o750)).unwrap();
    std::fs::write(config.join("admin.toml"), format!("remote_admin = \"off\"\nnode_user = \"{node_user}\"\n")).unwrap();
    k2_node::admin::jobs::write_jobs_file(&config, "nobody", &node_user).expect("nobody is a job user");
    std::fs::create_dir_all(base.join("bin")).unwrap();
    let bin = base.join("bin/k2-node");
    std::fs::copy(env!("CARGO_BIN_EXE_k2-node"), &bin).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let socket = PathBuf::from(format!("/run/k2na-test-{}.sock", &id[..10]));
    let exec = Executor::new(
        ExecPaths { config_dir: config, state_dir: state, home: base.join("home"), binary: bin.clone() },
        Arc::new(SystemPlatform),
    );
    let l = server::bind(&socket, None).unwrap();
    std::thread::spawn(move || {
        let _ = server::serve(&exec, &PeerRule { allowed_uids: vec![0] }, l, &|| false);
    });
    Rig { base, socket, bin }
}

async fn walled_node(fake: &Fake, r: &Rig, policy_extra: &str) -> TestNode {
    let sock = r.socket.clone();
    start_node_in(fake, r.base.clone(), policy_extra, move |o| {
        o.wall = Some(Wall::JobUser { user: "nobody".into() });
        o.admin_socket = sock;
        o.unit_poll = std::time::Duration::from_millis(100);
    })
    .await
}

fn probe_argv(bin: &Path, reads: &[PathBuf], lists: &[PathBuf], connects: &[String]) -> Vec<String> {
    let mut v = vec![bin.display().to_string(), "wall-probe".to_string()];
    for (flag, items) in [("--read", reads.iter().map(|p| p.display().to_string()).collect::<Vec<_>>()), ("--list", lists.iter().map(|p| p.display().to_string()).collect())] {
        for i in items {
            v.push(flag.into());
            v.push(i);
        }
    }
    for c in connects {
        v.push("--connect".into());
        v.push(c.clone());
    }
    v
}

fn probe_direct(argv: &[String]) -> serde_json::Value {
    let out = std::process::Command::new(&argv[0]).args(&argv[1..]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

fn probe_json(fr: &[Frame], job: &str) -> serde_json::Value {
    let out = text(&logs_of(fr, job), LogStream::Out);
    let err = text(&logs_of(fr, job), LogStream::Err);
    serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("probe output isn't JSON ({e}): out={out:?} err={err:?}"))
}

fn ok(v: &serde_json::Value, kind: &str, key: &str) -> bool {
    let s = v[kind][key].as_str().unwrap_or_else(|| panic!("{kind} {key} missing in {v}"));
    s == "ok" || s.starts_with("ok:")
}

fn last(fr: &[Frame], job: &str) -> k2_node_proto::frames::StateUpdate {
    states_of(fr, job).into_iter().last().expect("a state")
}

fn unit_loaded(unit: &str) -> bool {
    let out = std::process::Command::new("systemctl").args(["show", "-p", "LoadState", "--value", unit]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim() == "loaded"
}

fn nobody_uid() -> u32 {
    server::user_ids("nobody").expect("nobody exists").0
}

/// V15/V16: as the job user, in the unit, a job reads neither the node's
/// key (made world-readable here on purpose: the namespace wall holds even
/// when permissions wouldn't), its ledger, the owner's config nor the
/// executor's state, and /home and /root are closed. The control (same
/// probe, no wall) reaches every one.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "root + systemd: run on a Linux box with --ignored"]
async fn a_job_runs_as_the_job_user_and_cant_read_the_node_or_homes() {
    let r = rig("read");
    let mut fake = Fake::start().await;
    let n = walled_node(&fake, &r, "").await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    let offer = n.node.offer(&n.node.owner());
    assert!(offer.features.iter().any(|f| f == "job-wall") && offer.caps.hard, "{:?}", offer.features);
    std::fs::set_permissions(n.layout.state(), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(n.layout.key(), std::fs::Permissions::from_mode(0o644)).unwrap();
    let secret_state = r.base.join("admin-state/nonces.json");
    std::fs::write(&secret_state, "{}").unwrap();
    let reads = vec![n.layout.key(), n.layout.ledger(), n.layout.policy(), secret_state.clone()];
    let lists = vec![n.layout.state(), PathBuf::from("/home"), PathBuf::from("/root")];
    let argv = probe_argv(&r.bin, &reads, &lists, &[]);
    let control = probe_direct(&argv);
    for p in &reads {
        assert!(ok(&control, "read", &p.display().to_string()), "control: {control}");
    }
    for p in &lists {
        assert!(ok(&control, "list", &p.display().to_string()), "control: {control}");
    }
    let argv_s: Vec<&str> = argv.iter().map(String::as_str).collect();
    c.send(Frame::Assign(assign(plan("rd1", &argv_s), 1)));
    let fr = run_to_receipt(&mut c, "rd1").await;
    assert_eq!(last(&fr, "rd1").state, JobState::Done, "{fr:?}");
    let v = probe_json(&fr, "rd1");
    assert_eq!(v["uid"].as_u64(), Some(nobody_uid() as u64), "the job runs as the job user: {v}");
    for p in &reads {
        assert!(!ok(&v, "read", &p.display().to_string()), "the wall let the job read {}: {v}", p.display());
    }
    for p in &lists {
        assert!(!ok(&v, "list", &p.display().to_string()), "the wall let the job list {}: {v}", p.display());
    }
    assert!(!unit_loaded("k2job-rd1-g1.service"), "released after the job");
}

/// V21: `network = "none"` puts the job in its own network namespace: a
/// socket to this machine's public address fails (the control connects).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "root + systemd: run on a Linux box with --ignored"]
async fn network_none_blocks_sockets() {
    let r = rig("net");
    let mut fake = Fake::start().await;
    let n = walled_node(&fake, &r, "network = \"none\"\n").await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    assert_eq!(n.node.offer(&n.node.owner()).refusal, None, "a walled node takes network = none jobs");
    let s = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    s.connect("192.0.2.1:9").expect("a default route");
    let ip = s.local_addr().unwrap().ip();
    let lan = std::net::TcpListener::bind((ip, 0)).unwrap();
    let addr = lan.local_addr().unwrap().to_string();
    let argv = probe_argv(&r.bin, &[], &[], &[addr.clone()]);
    assert!(ok(&probe_direct(&argv), "connect", &addr), "control connects to {addr}");
    let argv_s: Vec<&str> = argv.iter().map(String::as_str).collect();
    c.send(Frame::Assign(assign(plan("net1", &argv_s), 1)));
    let fr = run_to_receipt(&mut c, "net1").await;
    let v = probe_json(&fr, "net1");
    assert!(v["connect"][&addr].as_str().unwrap().starts_with("error:"), "network none let a socket out: {v}");
    // The same node with network = full reaches it (the wall alone isn't
    // what blocks it).
    drop(n);
    let r2 = rig("net-full");
    let n2 = walled_node(&fake, &r2, "").await;
    let mut c2 = fake.next_conn().await;
    ready(&mut c2).await;
    let argv2 = probe_argv(&r2.bin, &[], &[], &[addr.clone()]);
    let argv2_s: Vec<&str> = argv2.iter().map(String::as_str).collect();
    c2.send(Frame::Assign(assign(plan("net2", &argv2_s), 1)));
    let fr = run_to_receipt(&mut c2, "net2").await;
    assert!(ok(&probe_json(&fr, "net2"), "connect", &addr), "network = full connects");
    drop(n2);
}

/// V24: a job over its memory cap is OOM-killed by the kernel and the
/// node reports `failed / mem_cap` with the cap and the `--mem` hint.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "root + systemd: run on a Linux box with --ignored"]
async fn an_oom_kill_is_reported_as_mem_cap() {
    let r = rig("oom");
    let mut fake = Fake::start().await;
    let _n = walled_node(&fake, &r, "").await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    // `tail` keeps the last 10 lines; /dev/zero has none, so it holds it all.
    let mut p = plan("oom1", &["/bin/sh", "-c", "head -c 1500000000 /dev/zero | tail; echo survived"]);
    p.limits.mem_bytes = Some(64 << 20);
    c.send(Frame::Assign(assign(p, 1)));
    let fr = run_to_receipt(&mut c, "oom1").await;
    let s = last(&fr, "oom1");
    assert_eq!((s.state, s.reason.as_deref()), (JobState::Failed, Some("mem_cap")), "{s:?}");
    assert!(s.detail.as_deref().unwrap_or("").contains("--mem"), "{s:?}");
    assert!(s.detail.as_deref().unwrap_or("").contains("64 MiB"), "{s:?}");
    assert!(text(&logs_of(&fr, "oom1"), LogStream::Sys).contains("memory cap"), "the log says why");
}

/// Security review B, F1: a job's /tmp and /var/tmp are its own folders in
/// its job root (no PrivateTmp outside the slot): what it writes there is
/// in the slot's measured bytes, another job doesn't see it, and it goes
/// with the job.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "root + systemd: run on a Linux box with --ignored"]
async fn a_jobs_tmp_is_in_its_slot_and_its_own() {
    let r = rig("tmp");
    let mut fake = Fake::start().await;
    let n = walled_node(&fake, &r, "max_parallel = 2\n").await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    c.send(Frame::Assign(assign(
        plan("t1", &["/bin/sh", "-c", "head -c 1048576 /dev/zero > /tmp/big && head -c 1048576 /dev/zero > /var/tmp/big2 && echo written && exec sleep 1000"]),
        1,
    )));
    c.until(|f| matches!(f, Frame::Log(l) if l.job_id == "t1")).await;
    let dir = n.layout.job_dir("t1", 1);
    let size = |p: std::path::PathBuf| std::fs::metadata(&p).map(|m| m.len()).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    assert_eq!(size(dir.join("tmp/big")), 1 << 20, "/tmp is the job's tmp folder");
    assert_eq!(size(dir.join("var-tmp/big2")), 1 << 20, "/var/tmp is the job's var-tmp folder");
    // Another job: its own, empty /tmp and /var/tmp.
    c.send(Frame::Assign(assign(plan("t2", &["/bin/sh", "-c", "ls -A /tmp /var/tmp; echo listed"]), 1)));
    let fr = run_to_receipt(&mut c, "t2").await;
    let out = text(&logs_of(&fr, "t2"), LogStream::Out);
    assert!(!out.contains("big"), "another job never sees it: {out}");
    c.send(Frame::Cancel(Cancel { job_id: "t1".into(), generation: 1, reason: "test".into() }));
    let fr = run_to_receipt(&mut c, "t1").await;
    assert_eq!(last(&fr, "t1").state, JobState::Cancelled);
    let t0 = std::time::Instant::now();
    while dir.join("tmp").exists() || dir.join("var-tmp").exists() {
        assert!(t0.elapsed() < std::time::Duration::from_secs(20), "its tmp goes with the job");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Exit codes, both streams and a cancel go through the unit; the unit is
/// gone afterwards, and the job's own dirs were writable.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "root + systemd: run on a Linux box with --ignored"]
async fn exit_logs_cancel_and_the_jobs_own_dirs() {
    let r = rig("exit");
    let mut fake = Fake::start().await;
    let n = walled_node(&fake, &r, "").await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    c.send(Frame::Assign(assign(
        plan("ex1", &["/bin/sh", "-c", "echo out; echo err >&2; id -un; echo x > \"$HOME/f\" && echo x > \"$TMPDIR/f\" && echo x > ./cwdfile && echo wrote; exit 3"]),
        1,
    )));
    let fr = run_to_receipt(&mut c, "ex1").await;
    let s = last(&fr, "ex1");
    assert_eq!((s.state, s.exit.as_ref().and_then(|e| e.code)), (JobState::Done, Some(3)), "{s:?}");
    let out = text(&logs_of(&fr, "ex1"), LogStream::Out);
    assert_eq!(out, "out\nnobody\nwrote\n", "{out}");
    assert_eq!(text(&logs_of(&fr, "ex1"), LogStream::Err), "err\n");
    // The node's journal can't be swapped by the job (sticky job root).
    c.send(Frame::Assign(assign(plan("ex2", &["/bin/sh", "-c", "mv log log.x 2>&1 || echo refused; rm -f log 2>&1 || true; ls log"]), 1)));
    let fr = run_to_receipt(&mut c, "ex2").await;
    let out = text(&logs_of(&fr, "ex2"), LogStream::Out);
    assert!(out.contains("refused") && out.trim_end().ends_with("log"), "the journal stays the node's: {out}");
    // Cancel: SIGTERM through the unit, then gone.
    c.send(Frame::Assign(assign(plan("ex3", &["/bin/sh", "-c", "echo up; exec sleep 1000"]), 1)));
    c.until(|f| matches!(f, Frame::Log(l) if l.job_id == "ex3")).await;
    assert!(unit_loaded("k2job-ex3-g1.service"));
    c.send(Frame::Cancel(Cancel { job_id: "ex3".into(), generation: 1, reason: "test".into() }));
    let fr = run_to_receipt(&mut c, "ex3").await;
    assert_eq!(last(&fr, "ex3").state, JobState::Cancelled);
    assert!(!unit_loaded("k2job-ex3-g1.service"), "released");
    // The executor takes a job tree back for the node (here: root anyway,
    // so this checks the verb works end to end on a real tree).
    n.node.reclaim_blocking(0, &n.layout.job_dir("ex1", 1));
    assert!(std::fs::read_to_string(n.layout.job_dir("ex1", 1).join("home/f")).is_ok());
}
