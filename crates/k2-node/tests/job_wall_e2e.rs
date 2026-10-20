//! The job wall (0.46.3, N3) end to end against the loopback fake
//! controller. Every wall test runs a control first (the same probe
//! without the wall reaches the thing), so a pass means the wall made
//! the difference, not the machine.
//!
//! - Any OS: an unwalled node (an older setup) still refuses jobs under
//!   `network = "none"`, and offers no wall features.
//! - macOS: the Seatbelt profile keeps a job off the node's key, state and
//!   config, `/Users`, and (network none) every non-loopback address.
//!
//! The Linux `k2job` unit needs root and systemd: `job_wall_linux.rs`.

mod common;

use std::path::{Path, PathBuf};

use common::*;
use k2_node::wall::Wall;
use k2_node_proto::frames::{self, Frame, JobState, LogStream};

fn last_state(fr: &[Frame], job: &str) -> frames::StateUpdate {
    states_of(fr, job).into_iter().last().expect("a state for the job")
}

/// The k2-node binary, copied out of the build tree (which may sit under
/// `/Users`, which the wall closes) into a temp dir.
fn probe_bin() -> PathBuf {
    let d = k2_node::util::temp_dir("probe-bin");
    let p = d.join("k2-node");
    std::fs::copy(env!("CARGO_BIN_EXE_k2-node"), &p).expect("copy the k2-node binary");
    p
}

fn probe_argv(bin: &Path, reads: &[PathBuf], lists: &[PathBuf], connects: &[String]) -> Vec<String> {
    let mut v = vec![bin.display().to_string(), "wall-probe".to_string()];
    for r in reads {
        v.push("--read".into());
        v.push(r.display().to_string());
    }
    for l in lists {
        v.push("--list".into());
        v.push(l.display().to_string());
    }
    for c in connects {
        v.push("--connect".into());
        v.push(c.clone());
    }
    v
}

/// The probe run directly (no wall), as the control.
fn probe_direct(argv: &[String]) -> serde_json::Value {
    let out = std::process::Command::new(&argv[0]).args(&argv[1..]).output().expect("run the probe");
    assert!(out.status.success(), "probe: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).expect("probe JSON")
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

/// This machine's address on its default route (not loopback). No packet
/// is sent: a UDP connect only picks the route.
fn lan_ip() -> std::net::IpAddr {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").expect("udp bind");
    s.connect("192.0.2.1:9").expect("this test needs a default route (a LAN address to listen on)");
    let ip = s.local_addr().unwrap().ip();
    assert!(!ip.is_loopback() && !ip.is_unspecified(), "no LAN address: {ip}");
    ip
}

/// Mixed fleet: a node without the wall (an older setup, or the wall not
/// set up yet) keeps refusing every job under `network = "none"`, with the
/// stable code, and never claims the wall in its offer.
#[tokio::test(flavor = "multi_thread")]
async fn an_unwalled_node_refuses_network_none_jobs_and_offers_no_wall() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "wall-none", "network = \"none\"\n", |o| o.wall = Some(Wall::None { why: "test".into() })).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    let offer = n.node.offer();
    assert!(!offer.features.iter().any(|f| f == frames::FEATURE_JOB_WALL || f == frames::FEATURE_NETWORK_NONE), "{:?}", offer.features);
    assert_eq!(offer.refusal.as_ref().map(|r| r.code.as_str()), Some("network_none_not_enforced_yet"));
    c.send(Frame::Assign(assign(plan("nn1", &["/bin/true"]), 1)));
    let fr = c.until(|f| matches!(f, Frame::State(s) if s.job_id == "nn1")).await;
    let s = last_state(&fr, "nn1");
    assert_eq!((s.state, s.reason.as_deref()), (JobState::Queued, Some("network_none_not_enforced_yet")));
    assert!(n.node.ledger.lock().unwrap().get("nn1", 1).unwrap().is_none(), "never ran");
    let st = k2_node::status::read(&n.layout.status()).unwrap();
    assert_eq!((st.wall.as_str(), st.network_none), ("none", false), "status.json tells the machine's K2");
}

#[cfg(target_os = "macos")]
mod seatbelt {
    use super::*;

    /// V15/V16: under Seatbelt a job reads neither the node's key, ledger
    /// or config nor anything under /Users; the same probe without the
    /// wall reads them all. The job's own dirs still work.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_job_cant_read_the_node_key_state_config_or_homes() {
        let mut fake = Fake::start().await;
        let n = start_node(&fake, "wall-sb", "", |o| o.wall = Some(Wall::Seatbelt)).await;
        let mut c = fake.next_conn().await;
        ready(&mut c).await;
        let offer = n.node.offer();
        assert!(offer.features.iter().any(|f| f == frames::FEATURE_JOB_WALL), "{:?}", offer.features);
        let bin = probe_bin();
        let key = n.layout.key();
        let ledger = n.layout.ledger();
        let policy = n.layout.policy();
        let own = n.layout.job_dir("sb1", 1).join("home");
        let argv = probe_argv(&bin, &[key.clone(), ledger.clone(), policy.clone()], &[n.layout.state(), PathBuf::from("/Users"), own.clone()], &[]);
        let control = probe_direct(&argv);
        for (kind, p) in [("read", &key), ("read", &ledger), ("read", &policy), ("list", &n.layout.state())] {
            assert!(ok(&control, kind, &p.display().to_string()), "control can't reach {}: {control}", p.display());
        }
        assert!(ok(&control, "list", "/Users"), "control lists /Users: {control}");
        let argv_s: Vec<&str> = argv.iter().map(String::as_str).collect();
        c.send(Frame::Assign(assign(plan("sb1", &argv_s), 1)));
        let fr = run_to_receipt(&mut c, "sb1").await;
        assert_eq!(last_state(&fr, "sb1").state, JobState::Done, "{fr:?}");
        let v = probe_json(&fr, "sb1");
        for (kind, p) in [("read", &key), ("read", &ledger), ("read", &policy), ("list", &n.layout.state())] {
            assert!(!ok(&v, kind, &p.display().to_string()), "the wall let the job reach {}: {v}", p.display());
        }
        assert!(!ok(&v, "list", "/Users"), "the wall let the job list /Users: {v}");
        assert!(ok(&v, "list", &own.display().to_string()), "the job's own home works: {v}");
    }

    /// V21: `network = "none"` under Seatbelt: a socket to this machine's
    /// own LAN address is refused (the control connects), loopback works.
    #[tokio::test(flavor = "multi_thread")]
    async fn network_none_blocks_sockets_but_keeps_loopback() {
        let mut fake = Fake::start().await;
        let n = start_node(&fake, "wall-sb-net", "network = \"none\"\n", |o| o.wall = Some(Wall::Seatbelt)).await;
        let mut c = fake.next_conn().await;
        ready(&mut c).await;
        let offer = n.node.offer();
        assert!(offer.features.iter().any(|f| f == frames::FEATURE_NETWORK_NONE), "{:?}", offer.features);
        assert_eq!(offer.refusal, None, "a walled node takes network = none jobs");
        let lan = std::net::TcpListener::bind((lan_ip(), 0)).unwrap();
        let lo = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let (lan_a, lo_a) = (lan.local_addr().unwrap().to_string(), lo.local_addr().unwrap().to_string());
        let bin = probe_bin();
        let argv = probe_argv(&bin, &[], &[], &[lan_a.clone(), lo_a.clone()]);
        let control = probe_direct(&argv);
        assert!(ok(&control, "connect", &lan_a) && ok(&control, "connect", &lo_a), "control connects: {control}");
        let argv_s: Vec<&str> = argv.iter().map(String::as_str).collect();
        c.send(Frame::Assign(assign(plan("net1", &argv_s), 1)));
        let fr = run_to_receipt(&mut c, "net1").await;
        let v = probe_json(&fr, "net1");
        let lan_err = v["connect"][&lan_a].as_str().unwrap();
        assert!(lan_err.starts_with("error:"), "network none let a socket out: {v}");
        assert!(ok(&v, "connect", &lo_a), "loopback stays: {v}");
        // network = full on the same wall: the LAN works.
        drop(n);
        let n2 = start_node(&fake, "wall-sb-full", "", |o| o.wall = Some(Wall::Seatbelt)).await;
        let mut c2 = fake.next_conn().await;
        ready(&mut c2).await;
        c2.send(Frame::Assign(assign(plan("net2", &argv_s), 1)));
        let fr = run_to_receipt(&mut c2, "net2").await;
        assert!(ok(&probe_json(&fr, "net2"), "connect", &lan_a), "network = full keeps the LAN");
        drop(n2);
    }
}
