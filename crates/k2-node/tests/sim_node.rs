//! iPhone sessions through a whole node (prd-ios-on-mac-node-0467-v1 S2),
//! with no Mac: the loopback fake server, a real k2-node, and a real
//! simhost on a temp socket whose `simctl` is recorded. Everything on the
//! path is real but the Simulator: the attach frames, admission, the
//! offer, the stream dial and its encrypted handshake, the relay, the
//! node's policy, the lease's grace, reconcile and close.
// pending k2-node Windows port: k2-node (the compute node runtime) is Unix-only
// until its Windows port (Phase 3: Job Objects instead of uids and cgroups).
#![cfg(unix)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::sim::{accept, call, sid, simctl as run, state};
use common::*;
use futures_util::SinkExt;
use k2_node::sim::exec::{Exec, ExecError, Out};
use k2_node::sim::helperbin::{HelperPipes, HelperSpawner};
use k2_node::sim::host::Host;
use k2_node::sim::node_side::SimOptions;
use k2_node::sim::switch::{self, SimSwitch};
use k2_node::sim::SimRoot;
use k2_node_proto::frames::Frame;
use k2_node_proto::sim::ctl::{ReplyBody, Request};
use k2_node_proto::sim::{codes, SessionState, SimClose, SimOpen, SimSync, FEATURE_SIM_IOS};
use tokio_tungstenite::tungstenite::Message;

const KEY: &str = "0123456789abcdef01234567";
const UDID: &str = "AD6DB1AF-E89F-48D1-A6D7-DDEF9C68B1EB";
/// What a test session reserves: the smallest a server may ask, so a small
/// test machine's free memory never decides the test.
const MEM: u64 = 1 << 30;

/// A Mac with Xcode 26.6, iOS 26.5 and an iPhone 17 booted; every call recorded.
#[derive(Default)]
struct Mac {
    calls: Mutex<Vec<Vec<String>>>,
}

impl Exec for Mac {
    fn run(&self, program: &str, args: &[String], _t: Duration) -> Result<Out, ExecError> {
        let mut rec = vec![program.to_string()];
        rec.extend(args.iter().cloned());
        self.calls.lock().unwrap().push(rec);
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        let ok = |s: String| Ok(Out { status: 0, stdout: s, stderr: String::new() });
        match (program, a.as_slice()) {
            ("xcrun", ["xcodebuild", "-version"]) => ok("Xcode 26.6\nBuild version 17F113\n".into()),
            ("/usr/bin/xcode-select", ["-p"]) => ok("/Applications/Xcode.app/Contents/Developer\n".into()),
            ("xcrun", ["simctl", "list", "-j", "runtimes"]) => ok(serde_json::json!({"runtimes": [{
                "platform": "iOS", "isAvailable": true, "identifier": "com.apple.CoreSimulator.SimRuntime.iOS-26-5", "version": "26.5", "name": "iOS 26.5",
                "supportedDeviceTypes": [{"name": "iPhone 17", "identifier": "com.apple.CoreSimulator.SimDeviceType.iPhone-17", "productFamily": "iPhone",
                    "bundlePath": "/Library/Developer/CoreSimulator/Profiles/DeviceTypes/iPhone 17.simdevicetype"}]}]})
            .to_string()),
            ("/usr/bin/plutil", ["-convert", "json", ..]) => ok(r#"{"mainScreenWidth":1206,"mainScreenHeight":2622,"mainScreenScale":3}"#.into()),
            ("xcrun", ["simctl", "--set", _, "list", "-j", "devices"]) => {
                ok(serde_json::json!({"devices": {"r": [{"udid": UDID, "name": format!("K2-{KEY}--iPhone-17"), "state": "Booted"}]}}).to_string())
            }
            ("/bin/ps", _) => ok(format!("{} 1024\n", unsafe { libc::getuid() })),
            _ => ok(String::new()),
        }
    }
}

struct NoHelpers;
impl HelperSpawner for NoHelpers {
    fn spawn(&self, _set: &std::path::Path, _udid: &str) -> Result<HelperPipes, String> {
        Err("no helper in this test".into())
    }
}

struct Rig {
    fake: Fake,
    node: TestNode,
    conn: Conn,
    mac: Arc<Mac>,
    root: std::path::PathBuf,
}

async fn rig(tag: &str, on: bool) -> Rig {
    let mut fake = Fake::start().await;
    let dir = k2_node::util::temp_dir(tag);
    let root = dir.join("k2sim");
    // A short name: a job's temp dir is too long for a Unix socket.
    let sock = std::path::PathBuf::from(format!("/tmp/k2sn-{tag}-{}.sock", std::process::id()));
    let mac = Arc::new(Mac::default());
    let host = Host::new(SimRoot::new(&root), mac.clone(), Arc::new(NoHelpers), unsafe { libc::getuid() }, Box::new(|_| Ok("/bin/true".into())));
    let l = host.bind(&sock).unwrap();
    std::thread::spawn(move || host.serve(l));
    let (r2, s2) = (root.clone(), sock.clone());
    let node = start_node(&fake, &format!("{tag}-node"), "", move |o| {
        o.sim = SimOptions { mac: true, root: SimRoot::new(&r2), socket: s2, tick: Duration::from_millis(100), probe_every: Duration::from_secs(3600) };
    })
    .await;
    if on {
        switch::write(&node.layout.config, &SimSwitch { slots: vec![0] }).unwrap();
    }
    let conn = fake.next_conn().await;
    Rig { fake, node, conn, mac, root }
}

fn open(n: u8) -> Frame {
    Frame::SimOpen(SimOpen { session: sid(n), workspace_key: KEY.into(), mem_bytes: MEM, lease_secs: 3600 })
}

#[tokio::test(flavor = "multi_thread")]
async fn an_iphone_session_runs_through_the_node_and_its_lease_holds() {
    let mut r = rig("simnode", true).await;
    // The offer says sim-ios with ready facts and this server's own paths.
    let offers = r.conn.until(|f| matches!(f, Frame::Offer(o) if o.sim.as_ref().is_some_and(|s| s.ready))).await;
    let Some(Frame::Offer(o)) = offers.last() else { unreachable!() };
    assert!(o.features.iter().any(|f| f == FEATURE_SIM_IOS));
    let facts = o.sim.clone().unwrap();
    assert_eq!(facts.xcode.as_deref(), Some("26.6 (17F113)"));
    assert_eq!(facts.runtimes[0].device_types[0].name, "iPhone 17");
    let set = r.root.join("sets/0").to_string_lossy().into_owned();
    assert_eq!(facts.paths.set, set);

    r.conn.send(open(1));
    assert_eq!(state(&mut r.conn, &sid(1)).await, (SessionState::Opening, None));
    let (mut ws, mut s, mut op) = accept(&mut r.fake, &r.node).await;
    assert_eq!(state(&mut r.conn, &sid(1)).await, (SessionState::Open, None));
    assert_eq!(r.node.node.sim.reserved_bytes(), MEM, "the session reserves its memory (IR-8)");
    // (The offer that says so went out beside the state frames above.)
    let owner = r.node.node.owner();
    assert_eq!(r.node.node.offer(&owner).sim.unwrap().sessions, vec![sid(1)], "the offer lists the live session (IR-7)");
    let st = k2_node::status::read(&r.node.layout.status()).unwrap();
    assert!(st.running.contains(&format!("sim:{}", sid(1))), "updates and drains wait for it: {:?}", st.running);

    // An allowed simctl call reaches the Mac; a refused one never does.
    let (b, recorded) = call(&mut ws, &mut s, &mut op, &Request::Run { id: 1, program: "xcrun".into(), args: run(&set, &["boot", UDID]), timeout_ms: 5000 }, Duration::from_secs(20)).await;
    assert!(matches!(b, ReplyBody::Run { status: 0, .. }), "{b:?}");
    let (b, _) = call(&mut ws, &mut s, &mut op, &Request::Run { id: 2, program: "xcrun".into(), args: run(&set, &["spawn", UDID, "sh", "-c", "id"]), timeout_ms: 5000 }, Duration::from_secs(20)).await;
    match b {
        ReplyBody::Refused { code, .. } => assert_eq!(code, codes::NODE_REFUSED),
        other => panic!("must be refused: {other:?}"),
    }
    let calls = r.mac.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|c| c.get(4).map(String::as_str) == Some("boot")));
    assert!(!calls.iter().any(|c| c.get(4).map(String::as_str) == Some("spawn")));

    // The stream drops: the device is kept, the node redials in the grace.
    drop(ws);
    assert_eq!(state(&mut r.conn, &sid(1)).await, (SessionState::Reconnecting, None));
    let (mut ws2, _s2, _o2) = accept(&mut r.fake, &r.node).await;
    assert_eq!(state(&mut r.conn, &sid(1)).await, (SessionState::Open, None));
    // A message recorded on the first connection doesn't open on the second (IR-2).
    ws2.send(Message::Binary(recorded)).await.unwrap();
    assert_eq!(state(&mut r.conn, &sid(1)).await, (SessionState::Reconnecting, None), "the replay closed the stream");
    let (_ws3, _s3, _o3) = accept(&mut r.fake, &r.node).await;
    assert_eq!(state(&mut r.conn, &sid(1)).await, (SessionState::Open, None));

    // A second iPhone on the node: busy (one per node, Q10).
    r.conn.send(Frame::SimOpen(SimOpen { session: sid(2), workspace_key: "ffffffffffffffffffffffff".into(), mem_bytes: MEM, lease_secs: 3600 }));
    assert_eq!(state(&mut r.conn, &sid(2)).await, (SessionState::Refused, Some(codes::NODE_BUSY.into())));

    // The server ends it: closed, and the device is shut down.
    r.conn.send(Frame::SimClose(SimClose { session: sid(1), reason: "stop".into() }));
    assert_eq!(state(&mut r.conn, &sid(1)).await, (SessionState::Closed, Some(codes::CLOSED.into())));
    let t0 = std::time::Instant::now();
    while !r.mac.calls.lock().unwrap().iter().any(|c| c.get(4).map(String::as_str) == Some("shutdown")) {
        assert!(t0.elapsed() < Duration::from_secs(10), "no shutdown reached the Mac");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(r.node.node.sim.reserved_bytes(), 0);

    // After a server restart, sim_sync frees the node at once (IR-7).
    r.conn.send(open(3));
    assert_eq!(state(&mut r.conn, &sid(3)).await, (SessionState::Opening, None));
    r.conn.send(Frame::SimSync(SimSync { sessions: vec![] }));
    assert_eq!(state(&mut r.conn, &sid(3)).await, (SessionState::Closed, Some(codes::NOT_HELD.into())));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slot_the_owner_didnt_turn_on_gets_no_sim_and_is_refused() {
    let mut r = rig("simoff", false).await;
    let offers = r.conn.until(|f| matches!(f, Frame::Offer(_))).await;
    let Some(Frame::Offer(o)) = offers.last() else { unreachable!() };
    assert!(o.sim.is_none() && !o.features.iter().any(|f| f == FEATURE_SIM_IOS));
    r.conn.send(open(1));
    assert_eq!(state(&mut r.conn, &sid(1)).await, (SessionState::Refused, Some(codes::NOT_ENABLED.into())));
    // Turned on: the next offer has it.
    switch::write(&r.node.layout.config, &SimSwitch { slots: vec![0] }).unwrap();
    r.conn.until(|f| matches!(f, Frame::Offer(o) if o.features.iter().any(|x| x == FEATURE_SIM_IOS) && o.sim.as_ref().is_some_and(|s| s.ready))).await;
}
