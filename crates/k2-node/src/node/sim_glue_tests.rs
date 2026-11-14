//! The node's own view of iPhone sessions: who gets `sim-ios`, admission
//! against the node's state, the wall, and memory.

use std::sync::atomic::Ordering;

use k2_node_proto::frames::Frame;
use k2_node_proto::sim::{codes, SessionState, SimOpen, SimRuntime, SimDeviceType, FEATURE_SIM_IOS, PROTOCOL_SIM};

use super::super::*;
use crate::sim::local::HostFacts;
use crate::sim::node_side::SimOptions;
use crate::sim::switch::SimSwitch;
use crate::sim::SimRoot;

async fn node(tag: &str) -> Arc<Node> {
    let d = crate::util::temp_dir(tag);
    let mut o = NodeOptions::new(Layout::new(d.join("home"), d.join("cfg")));
    o.dev = true;
    o.health = false;
    o.keep_awake = false;
    o.use_cgroups = false;
    o.wall = Some(Wall::Seatbelt);
    o.sim = SimOptions { mac: true, root: SimRoot::new(d.join("k2sim")), socket: d.join("k2sim.sock"), ..SimOptions::system() };
    Node::open(o).await.unwrap()
}

fn ready() -> HostFacts {
    HostFacts {
        xcode: Some("26.6 (17F113)".into()),
        runtimes: vec![SimRuntime {
            identifier: "com.apple.CoreSimulator.SimRuntime.iOS-26-5".into(),
            name: "iOS 26.5".into(),
            version: "26.5".into(),
            device_types: vec![SimDeviceType { identifier: "t".into(), name: "iPhone 17".into(), width_pt: 402, height_pt: 874, scale_milli: 3000, default: true }],
        }],
        helper_built: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn sim_ios_goes_only_to_a_turned_on_slot_that_speaks_protocol_11() {
    let n = node("simglue-offer").await;
    let t = n.owner();
    t.protocol.store(PROTOCOL_SIM, Ordering::SeqCst);
    assert!(!n.sim_feature(&t) && n.offer(&t).sim.is_none(), "off until the owner turns it on");
    n.sim.set_switch(SimSwitch { slots: vec![0] });
    let o = n.offer(&t);
    assert!(o.features.iter().any(|f| f == FEATURE_SIM_IOS));
    let f = o.sim.expect("Offer.sim");
    assert!(!f.ready && f.needs[0].command == "sudo k2-node setup --sim", "simhost hasn't answered: not ready, and the step says so");
    n.sim.set_facts(Ok(ready()));
    assert!(n.offer(&t).sim.unwrap().ready);
    t.protocol.store(PROTOCOL_SIM - 1, Ordering::SeqCst);
    let o = n.offer(&t);
    assert!(o.sim.is_none() && !o.features.iter().any(|f| f == FEATURE_SIM_IOS), "an older server never sees a sim field");
}

#[tokio::test]
async fn a_sim_open_is_judged_against_the_nodes_own_state() {
    let n = node("simglue-admit").await;
    let t = n.owner();
    t.protocol.store(PROTOCOL_SIM, Ordering::SeqCst);
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    t.hub.attach(tx);
    let open = |s: u8| Frame::SimOpen(SimOpen { session: format!("{s:0>32x}"), workspace_key: "0123456789abcdef01234567".into(), mem_bytes: 1 << 30, lease_secs: 600 });
    let mut next = || match rx.try_recv() {
        Ok(Frame::SimState(s)) => (s.state, s.code),
        other => panic!("expected a sim_state, got {other:?}"),
    };
    n.on_sim_frame(&t, open(1));
    assert_eq!(next(), (SessionState::Refused, Some(codes::NOT_ENABLED.into())), "not turned on");
    n.sim.set_switch(SimSwitch { slots: vec![0] });
    n.on_sim_frame(&t, open(2));
    assert_eq!(next(), (SessionState::Refused, Some(codes::NOT_ENABLED.into())), "turned on but simhost isn't ready");
    n.sim.set_facts(Ok(ready()));
    n.local.lock().unwrap().control.control = Control::Draining;
    n.on_sim_frame(&t, open(3));
    assert_eq!(next(), (SessionState::Refused, Some(codes::NODE_BUSY.into())), "draining");
    n.local.lock().unwrap().control.control = Control::Active;
    n.local.lock().unwrap().locks.holding = Some("another team's VM job".into());
    let a = n.sim_admission(&t);
    assert_eq!(a.foreign_lock.as_deref(), Some("another team's VM job"), "a foreign lock holds new sessions");
    n.local.lock().unwrap().locks.holding = None;
    // Admitted: memory is reserved and jobs see less.
    let cap = n.local.lock().unwrap().policy.mem_cap_bytes;
    let before = n.sim_admission(&t).free_mem;
    n.sim.leases.lock().unwrap().open(0, &SimOpen { session: "f".repeat(32), workspace_key: "0123456789abcdef01234567".into(), mem_bytes: 1 << 30, lease_secs: 600 }, &k2_node_proto::sim::lease::Admission { enabled: true, free_mem: u64::MAX, disk_ok: true, ..Default::default() }, 0);
    assert_eq!(n.sim.default_job_mem(cap), cap - (1 << 30));
    let after = n.sim_admission(&t).free_mem;
    assert!(after < before || before < (1 << 30), "the reservation counts as used ({before} → {after})");
}

#[tokio::test]
async fn the_job_wall_closes_the_simulator_users_home_and_socket() {
    let n = node("simglue-wall").await;
    let (dirs, socks) = n.seatbelt_deny();
    assert!(dirs.contains(&n.sim.opts.root.root), "{dirs:?}");
    assert!(socks.contains(&n.sim.opts.socket), "{socks:?}");
    assert_eq!(SimOptions::system().socket, std::path::PathBuf::from("/var/k2sim/run/simhost.sock"));
}
