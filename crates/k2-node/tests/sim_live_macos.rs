//! Live: one whole iPhone session through k2-node and simhost on this Mac
//! (prd-ios-on-mac-node-0467-v1 S2, the node's half of S5). Needs a Mac
//! with Xcode and an iOS runtime. It only compiles with `--features
//! sim-live`, and then a missing Xcode is a failure, never a skip:
//!
//!   cargo test -p k2-node --features sim-live --test sim_live_macos -- --nocapture
//!
//! The server is the loopback fake; the node and simhost are real, simhost
//! runs as whoever runs the test (a compute job: the node's own job user)
//! with its own device set in a temp folder, and the Simulator is real:
//! create, boot, the helper's frames, a tap, the tree, Safari, a
//! screenshot through the node's folder, the lease's close and erase. The
//! devices are deleted at the end, even on a failure.

#![cfg(target_os = "macos")]

mod common;

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::sim::{accept, call, recv, send, sid, simctl, state};
use common::*;
use k2_node::sim::exec::{Exec, SystemExec};
use k2_node::sim::helperbin::{ensure_built, SystemHelpers};
use k2_node::sim::host::Host;
use k2_node::sim::node_side::SimOptions;
use k2_node::sim::switch::{self, SimSwitch};
use k2_node::sim::SimRoot;
use k2_node_proto::frames::Frame;
use k2_node_proto::sim::ctl::{DiskOp, ReplyBody, Request};
use k2_node_proto::sim::{codes, SessionState, SimClose, SimErase, SimOpen, FEATURE_SIM_IOS};
use serde_json::{json, Value as J};

const KEY: &str = "0a1b2c3d4e5f60718293a4b5";

/// Shuts down and deletes every device in the test's set, however it ends.
struct Cleanup {
    set: String,
    sock: PathBuf,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = Command::new("xcrun").args(["simctl", "--set", &self.set, "shutdown", "all"]).status();
        let _ = Command::new("xcrun").args(["simctl", "--set", &self.set, "delete", "all"]).status();
        let _ = std::fs::remove_file(&self.sock);
    }
}

fn devices(set: &str) -> Vec<(String, String)> {
    let o = Command::new("xcrun").args(["simctl", "--set", set, "list", "-j", "devices"]).output().expect("xcrun simctl");
    let v: J = serde_json::from_slice(&o.stdout).expect("simctl list devices JSON");
    v["devices"].as_object().unwrap().values().flat_map(|a| a.as_array().unwrap().clone()).map(|d| (d["udid"].as_str().unwrap().to_string(), d["state"].as_str().unwrap().to_string())).collect()
}

/// A helper's JSON message on its channel: `u32 len ‖ J ‖ json`.
fn helper_json(plain: &[u8]) -> Option<J> {
    (plain.get(4) == Some(&b'J')).then(|| serde_json::from_slice(&plain[5..]).ok()).flatten()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_whole_iphone_session_on_this_mac() {
    let t0 = Instant::now();
    let mut fake = Fake::start().await;
    let dir = k2_node::util::temp_dir("simlive");
    let root = dir.join("k2sim");
    // A short path: a Unix socket's name is at most 104 bytes on macOS.
    let sock = PathBuf::from(format!("/tmp/k2sl-{}.sock", std::process::id()));
    let sroot = SimRoot::new(&root);
    let set = sroot.set(0).to_string_lossy().into_owned();
    let _cleanup = Cleanup { set: set.clone(), sock: sock.clone() };

    let exec: Arc<dyn Exec> = Arc::new(SystemExec { home: root.clone(), tmp: dir.clone() });
    let helpers = Arc::new(SystemHelpers { bin_dir: sroot.bin(), home: root.clone(), tmp: dir.clone(), exec: exec.clone() });
    let (bin, x2) = (sroot.bin(), exec.clone());
    let host = Host::new(sroot.clone(), exec, helpers, unsafe { libc::getuid() }, Box::new(move |x| ensure_built(&bin, &*x2, x)));
    let l = host.bind(&sock).expect("bind simhost's socket");
    std::thread::spawn(move || host.serve(l));

    let (r2, s2) = (root.clone(), sock.clone());
    let node = start_node(&fake, "simlive-node", "", move |o| {
        o.offer_every = Duration::from_secs(2);
        o.sim = SimOptions { mac: true, root: SimRoot::new(&r2), socket: s2, tick: Duration::from_millis(250), probe_every: Duration::from_secs(3600) };
    })
    .await;
    switch::write(&node.layout.config, &SimSwitch { slots: vec![0] }).unwrap();
    let mut conn = fake.next_conn().await;

    // The offer: Xcode, a runtime with an iPhone, the helper built. A Mac
    // without them fails here, naming what's missing.
    let deadline = Instant::now() + Duration::from_secs(600);
    let facts = loop {
        let f = conn.next().await;
        if let Frame::Offer(o) = f {
            if let Some(s) = o.sim.as_ref() {
                assert!(o.features.iter().any(|x| x == FEATURE_SIM_IOS));
                if s.ready {
                    break s.clone();
                }
                if s.needs.iter().all(|n| !n.what.contains("not asked yet")) {
                    panic!("this Mac isn't ready for iPhones: {:?}", s.needs);
                }
            }
        }
        assert!(Instant::now() < deadline, "no ready sim facts within 10 minutes");
    };
    eprintln!("[live] ready after {:?}: Xcode {:?}", t0.elapsed(), facts.xcode);
    let rt = &facts.runtimes[0];
    let ty = rt.device_types.iter().find(|t| t.default).expect("a default iPhone");

    conn.send(Frame::SimOpen(SimOpen { session: sid(1), workspace_key: KEY.into(), mem_bytes: 1 << 30, lease_secs: 3600 }));
    assert_eq!(state(&mut conn, &sid(1)).await, (SessionState::Opening, None));
    let (mut ws, mut s, mut o) = accept(&mut fake, &node).await;
    assert_eq!(state(&mut conn, &sid(1)).await, (SessionState::Open, None));
    let t = Duration::from_secs(300);
    let mut id = 0u64;
    let mut next = || {
        id += 1;
        id
    };
    let run = |id: u64, rest: &[&str], ms: u64| Request::Run { id, program: "xcrun".into(), args: simctl(&set, rest), timeout_ms: ms };

    // Create and boot, as the server's iOS lane does.
    let name = format!("K2-{KEY}--{}", ty.name.replace(' ', "-"));
    let (b, _) = call(&mut ws, &mut s, &mut o, &run(next(), &["create", &name, &ty.identifier, &rt.identifier], 120_000), t).await;
    let ReplyBody::Run { status: 0, stdout, .. } = b else { panic!("create: {b:?}") };
    let udid = stdout.trim().to_string();
    let tb = Instant::now();
    let (b, _) = call(&mut ws, &mut s, &mut o, &run(next(), &["boot", &udid], 120_000), t).await;
    assert!(matches!(b, ReplyBody::Run { status: 0, .. }), "boot: {b:?}");
    let (b, _) = call(&mut ws, &mut s, &mut o, &run(next(), &["bootstatus", &udid, "-b"], 240_000), t).await;
    assert!(matches!(b, ReplyBody::Run { status: 0, .. }), "bootstatus: {b:?}");
    eprintln!("[live] booted in {:?}", tb.elapsed());

    // The helper: ready, hello, frames one at a time, a tap, the tree.
    let (b, _) = call(&mut ws, &mut s, &mut o, &Request::HelperStart { id: next(), channel: 1, udid: udid.clone() }, t).await;
    assert_eq!(b, ReplyBody::Ok, "helper_start");
    let helper = |id: u64, v: J| {
        let mut v = v;
        v["id"] = json!(id);
        format!("{v}\n").into_bytes()
    };
    let hid = next();
    send(&mut ws, &mut s, 1, &helper(hid, json!({"op": "hello"}))).await;
    let hello = loop {
        let (ch, p) = recv(&mut ws, &mut o, t).await;
        if let Some(v) = helper_json(&p).filter(|v| ch == 1 && v["id"] == hid) {
            break v;
        }
    };
    assert_eq!(hello["ok"], true, "{hello}");
    eprintln!("[live] helper hello: screen {} ", hello["screen"]);
    send(&mut ws, &mut s, 1, &helper(next(), json!({"op": "frames", "on": true, "fps": 8, "scale": 0.33, "quality": 0.7}))).await;
    let mut frames = 0;
    let tf = Instant::now();
    while frames < 3 {
        let (ch, p) = recv(&mut ws, &mut o, Duration::from_secs(30)).await;
        if ch == 1 && p.get(4) == Some(&b'F') {
            frames += 1;
            send(&mut ws, &mut s, 0, &serde_json::to_vec(&Request::Ack { channel: 1 }).unwrap()).await;
        }
    }
    eprintln!("[live] 3 frames in {:?}", tf.elapsed());
    // Safari on a public page; a file: URL is refused before it reaches simctl.
    let (b, _) = call(&mut ws, &mut s, &mut o, &run(next(), &["openurl", &udid, "https://example.com"], 60_000), t).await;
    assert!(matches!(b, ReplyBody::Run { status: 0, .. }), "openurl: {b:?}");
    let (b, _) = call(&mut ws, &mut s, &mut o, &run(next(), &["openurl", &udid, "file:///etc/hosts"], 60_000), t).await;
    assert!(matches!(&b, ReplyBody::Refused { code, .. } if code == codes::NODE_REFUSED), "{b:?}");

    // The tree of the app in front (Safari), once it has drawn.
    let deadline = Instant::now() + Duration::from_secs(60);
    let tree = loop {
        let tid = next();
        send(&mut ws, &mut s, 1, &helper(tid, json!({"op": "tree"}))).await;
        let v = loop {
            let (ch, p) = recv(&mut ws, &mut o, t).await;
            if ch == 1 && p.get(4) == Some(&b'F') {
                send(&mut ws, &mut s, 0, &serde_json::to_vec(&Request::Ack { channel: 1 }).unwrap()).await;
            }
            if let Some(v) = helper_json(&p).filter(|v| ch == 1 && v["id"] == tid) {
                break v;
            }
        };
        if v["code"] != "no_frontmost_app" || Instant::now() > deadline {
            break v;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    };
    assert_eq!(tree["ok"], true, "{tree}");
    let tap = next();
    send(&mut ws, &mut s, 1, &helper(tap, json!({"op": "tap", "x": 201, "y": 437}))).await;
    loop {
        let (ch, p) = recv(&mut ws, &mut o, t).await;
        if ch == 1 && p.get(4) == Some(&b'F') {
            send(&mut ws, &mut s, 0, &serde_json::to_vec(&Request::Ack { channel: 1 }).unwrap()).await;
        }
        if let Some(v) = helper_json(&p).filter(|v| ch == 1 && v["id"] == tap) {
            assert_eq!(v["ok"], true, "tap: {v}");
            break;
        }
    }

    // A screenshot into the workspace's folder on the node, read back.
    let ws_tmp = facts.paths.workspace_tmp(KEY);
    let (b, _) = call(&mut ws, &mut s, &mut o, &Request::Disk { id: next(), op: DiskOp::EnsureDir, path: ws_tmp.clone() }, t).await;
    assert_eq!(b, ReplyBody::Ok);
    let shot = format!("{ws_tmp}/.shot-live.png");
    let iid = next();
    send(&mut ws, &mut s, 1, &helper(iid, json!({"op": "image", "path": shot, "scale": 1.0}))).await;
    loop {
        let (ch, p) = recv(&mut ws, &mut o, t).await;
        if ch == 1 && p.get(4) == Some(&b'F') {
            send(&mut ws, &mut s, 0, &serde_json::to_vec(&Request::Ack { channel: 1 }).unwrap()).await;
        }
        if let Some(v) = helper_json(&p).filter(|v| ch == 1 && v["id"] == iid) {
            assert_eq!(v["ok"], true, "{v}");
            break;
        }
    }
    let (b, _) = call(&mut ws, &mut s, &mut o, &Request::Disk { id: next(), op: DiskOp::Read, path: shot }, t).await;
    let ReplyBody::Bytes { data } = b else { panic!("read: {b:?}") };
    let png = k2_node_proto::crypto::unb64(&data).unwrap();
    assert!(png.starts_with(b"\x89PNG"), "a PNG of {} bytes", png.len());

    // The server ends the lease: the device shuts down; erase deletes it.
    conn.send(Frame::SimClose(SimClose { session: sid(1), reason: "done".into() }));
    assert_eq!(state(&mut conn, &sid(1)).await, (SessionState::Closed, Some(codes::CLOSED.into())));
    let td = Instant::now();
    while devices(&set).iter().any(|(u, st)| u == &udid && st != "Shutdown") {
        assert!(td.elapsed() < Duration::from_secs(120), "the device didn't shut down: {:?}", devices(&set));
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    conn.send(Frame::SimErase(SimErase { workspace_key: KEY.into() }));
    while devices(&set).iter().any(|(u, _)| u == &udid) {
        assert!(td.elapsed() < Duration::from_secs(240), "erase didn't delete the device");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    eprintln!("[live] whole session in {:?}", t0.elapsed());
}
