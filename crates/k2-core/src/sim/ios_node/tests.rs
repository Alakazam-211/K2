//! The `ios-node` lane against a fake Mac node: the real attach frames,
//! stream crypto, node policy, leases and unpack; simctl and the helper
//! played in memory. Every test fails loudly.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use k2_node_proto::frames::Frame;
use k2_node_proto::sim::{self as psim, SimSync};
use serde_json::json;

use super::fake_node::{self, FakeDirectory, NODE_ID, NODE_NAME, TYPE_17_PRO};
use super::*;

const WS_ID: &str = "proj-web";
const CANARY: &str = "k2-canary-hunter2-9f3e";

fn key() -> String {
    crate::sim::workspace_key(WS_ID)
}

fn setup() -> (Arc<FakeDirectory>, Arc<IosNodeLane>) {
    let dir = FakeDirectory::new();
    dir.grant(WS_ID);
    let lane = Arc::new(IosNodeLane::new(dir.clone()).with_timeouts(Duration::from_secs(10), Duration::from_secs(120)));
    dir.node.attach_lane(&lane);
    (dir, lane)
}

fn req(profile: Option<&str>) -> BootRequest {
    BootRequest {
        workspace_id: WS_ID.into(),
        workspace_key: key(),
        profile_id: profile.map(str::to_string),
        // The server's own folder: it must never reach the node (IR-5).
        data_dir: PathBuf::from("/home/k2/.k2/sim/ios-node/devices/SERVER-ONLY"),
        boot_timeout: Duration::from_secs(30),
        grant_exempt: false,
    }
}

fn dref(b: &BootedDevice) -> DeviceRef {
    let r = req(None);
    DeviceRef { workspace_id: r.workspace_id, workspace_key: r.workspace_key, device_id: b.device_id.clone(), data_dir: r.data_dir, lane_state: b.lane_state.clone() }
}

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("k2-ios-node-{tag}-{}-{}", std::process::id(), k2_node_proto::crypto::random_id().unwrap()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn profiles_name_the_node_and_need_no_session() {
    let (_dir, lane) = setup();
    let p = lane.profiles().unwrap();
    assert_eq!(p[0].name, "iPhone 17 (on rig1-mac)");
    assert_eq!(p[0].id, format!("{NODE_NAME}:{}", fake_node::TYPE_17));
    assert!(p[0].default && p.iter().filter(|x| x.default).count() == 1);
    assert_eq!((p[0].width_pt, p[0].height_pt, p[0].scale), (402, 874, 3.0));
    let probe = lane.probe();
    assert!(probe.usable && probe.ready, "{probe:?}");
    assert_eq!(probe.lane, LaneKind::IosNode);
}

#[test]
fn a_fake_node_session_end_to_end() {
    let (dir, lane) = setup();
    let b = lane.boot(&req(None)).unwrap();
    assert_eq!(b.profile.name, "iPhone 17 (on rig1-mac)");
    assert_eq!(b.profile.id, format!("{NODE_NAME}:{}", fake_node::TYPE_17));
    assert_eq!(b.lane_state["node"], NODE_NAME);
    assert_eq!(lane.session_state(&key()), Some("open"));
    let d = dref(&b);
    assert!(lane.alive(&d));

    // Install the fixture app through the upload.
    let src = tmpdir("app");
    let app = fake_node::fixture_app(&src);
    let out = lane.install_app(&d, &app).unwrap();
    assert_eq!(out.app_id, "dev.k2.simfixture");
    let installed = dir.node.world.lock().unwrap().installed.clone();
    assert_eq!(installed.len(), 1);
    assert!(installed[0].starts_with("/var/k2sim/uploads/0/") && installed[0].ends_with("/K2Fixture.app"), "{installed:?}");

    lane.launch(&d, "dev.k2.simfixture").unwrap();
    let t = lane.tree(&d).unwrap();
    assert_eq!(t.roots.len(), 1);
    assert_eq!((t.width_pt, t.height_pt), (402.0, 874.0));
    lane.act(&d, &Action::Tap { at: Point { x: 200.0, y: 262.0 } }).unwrap();
    assert_eq!(dir.node.world.lock().unwrap().taps, vec![(200.0, 262.0)]);
    lane.act(&d, &Action::Type { text: SimText::secret(CANARY) }).unwrap();
    assert_eq!(dir.node.world.lock().unwrap().field, CANARY, "the secret reached the field");

    let shot = lane.screenshot(&d).unwrap();
    assert!(matches!(&shot.data, FrameData::Png(b) if b.starts_with(b"\x89PNG")), "{shot:?}");
    assert_eq!(shot.width_px, 1206);

    let mut frames = lane.frames(&d, FrameOpts { max_fps: 60, ..FrameOpts::default() }).unwrap();
    let f1 = frames.next_frame(Duration::from_secs(5)).unwrap().expect("a frame");
    assert!(matches!(&f1.data, FrameData::Jpeg(b) if b.starts_with(b"\xff\xd8")));
    // The fake sends its next frame only after the server's ack, up to its
    // budget of 3. The server's inbox is newest-wins, so a slow reader may
    // skip one; the last frame always arrives, which takes both acks.
    let mut last = f1.seq;
    while last < 3 {
        let f = frames.next_frame(Duration::from_secs(5)).unwrap().expect("the next frame after the ack");
        assert!(f.seq > last, "frames arrive in order: {} after {last}", f.seq);
        last = f.seq;
    }
    assert_eq!(last, 3, "every frame of the budget was sent, one per ack");
    drop(frames);

    lane.act(&d, &Action::OpenUrl { url: "https://web.app.k2.dev".into() }).unwrap();
    assert_eq!(dir.node.world.lock().unwrap().opened, vec!["https://web.app.k2.dev".to_string()]);
    let logs = lane.logs(&d, &LogQuery { app_id: Some("dev.k2.simfixture".into()), since: None, grep: None, max_lines: 10, max_bytes: 10_000 }).unwrap();
    assert_eq!(logs.len(), 1);

    // Nothing the server keeps for itself went to the node.
    let log = dir.node.log.lock().unwrap().clone();
    assert!(!log.iter().any(|l| l.contains("SERVER-ONLY")), "{log:?}");

    // The secret canary is never on the relay, nor in the node's log.
    assert!(!fake_node::relay_carries(&dir.node, CANARY.as_bytes()), "the relay leg carries the secret in plain");
    assert!(!fake_node::relay_carries(&dir.node, b"simctl"), "the relay leg carries a command in plain");
    assert!(!fake_node::relay_carries(&dir.node, b"<plist/>"), "the relay leg carries the uploaded app in plain");
    assert!(!log.iter().any(|l| l.contains(CANARY)), "the node logged the secret: {log:?}");
    assert!(log.iter().all(|l| l.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')), "the node logs op names only: {log:?}");

    // Stop: the node shuts the device down and frees the lease.
    lane.stop(&d).unwrap();
    assert!(dir.node.world.lock().unwrap().shutdowns.contains(&b.device_id));
    assert!(dir.node.sessions().is_empty());
    assert!(!lane.alive(&d));
    assert_eq!(lane.session_state(&key()), None);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn a_recorded_message_replayed_into_the_next_connection_closes_it() {
    let (dir, lane) = setup();
    let b = lane.boot(&req(None)).unwrap();
    let d = dref(&b);
    lane.tree(&d).unwrap();
    let session = dir.node.sessions()[0].clone();
    let recorded: Vec<u8> = dir.node.relay.lock().unwrap().iter().rev().find(|(c2n, _)| *c2n).map(|(_, m)| m.clone()).expect("a server → node message");
    let reflected: Vec<u8> = dir.node.relay.lock().unwrap().iter().rev().find(|(c2n, _)| !*c2n).map(|(_, m)| m.clone()).expect("a node → server message");
    // A redial: new keys for the same session.
    dir.node.drop_stream(&session);
    dir.node.dial(&session, &key()).unwrap();
    lane.tree(&d).unwrap();
    let e = dir.node.inject(&session, &recorded).unwrap_err();
    assert!(e.contains("doesn't open") || e.contains("sequence"), "{e}");
    assert_eq!(lane.session_state(&key()), Some("reconnecting"), "the replay closed the connection");
    // Reflection: a node → server message bounced back at the server.
    dir.node.dial(&session, &key()).unwrap();
    let conn = lane.conn_of(&key()).unwrap();
    assert!(conn.deliver(&reflected).is_err());
    assert!(conn.is_closed());
}

#[test]
fn the_reaper_keeps_a_reconnecting_device_inside_the_grace() {
    let (dir, lane) = setup();
    let b = lane.boot(&req(None)).unwrap();
    let d = dref(&b);
    let session = dir.node.sessions()[0].clone();
    dir.node.drop_stream(&session);
    assert_eq!(lane.session_state(&key()), Some("reconnecting"));
    assert!(lane.alive(&d), "IR-6: alive from state inside the grace, never from the node");
    let e = lane.tree(&d).unwrap_err();
    assert_eq!(e.code(), "sim_not_running");
    assert!(e.message().contains("reconnecting to rig1-mac"), "{}", e.message());
    // A redial inside the grace keeps the device.
    dir.node.dial(&session, &key()).unwrap();
    assert!(lane.alive(&d));
    lane.tree(&d).unwrap();
    assert!(dir.node.world.lock().unwrap().shutdowns.is_empty());
    // No redial: the node ends the lease and shuts the device down.
    dir.node.drop_stream(&session);
    dir.node.tick(psim::GRACE_SECS);
    assert!(dir.node.world.lock().unwrap().shutdowns.contains(&b.device_id));
    assert!(!lane.alive(&d));
    let e = lane.tree(&d).unwrap_err();
    assert!(e.message().contains("lost the connection"), "{}", e.message());
}

#[test]
fn the_server_side_grace_runs_out_too() {
    let dir = FakeDirectory::new();
    dir.grant(WS_ID);
    let lane = Arc::new(IosNodeLane::new(dir.clone()).with_timeouts(Duration::from_secs(10), Duration::from_millis(50)));
    dir.node.attach_lane(&lane);
    let d = dref(&lane.boot(&req(None)).unwrap());
    dir.node.drop_stream(&dir.node.sessions()[0].clone());
    assert!(lane.alive(&d));
    std::thread::sleep(Duration::from_millis(80));
    assert!(!lane.alive(&d), "past the grace the reaper may stop the row");
}

#[test]
fn sim_sync_after_a_server_restart_frees_the_node_at_once() {
    let (dir, lane) = setup();
    let b = lane.boot(&req(None)).unwrap();
    assert_eq!(dir.node.sessions().len(), 1);
    // The server restarts: a new lane holds nothing.
    drop(lane);
    let lane2 = Arc::new(IosNodeLane::new(dir.clone()).with_timeouts(Duration::from_secs(10), Duration::from_secs(120)));
    dir.node.attach_lane(&lane2);
    assert!(lane2.held_sessions(NODE_ID).is_empty());
    dir.node.on_frame(Frame::SimSync(SimSync { sessions: lane2.held_sessions(NODE_ID) }));
    assert!(dir.node.sessions().is_empty(), "IR-7: not busy for 120 s");
    assert!(dir.node.world.lock().unwrap().shutdowns.contains(&b.device_id));
    let again = lane2.boot(&req(None)).unwrap();
    assert_eq!(again.device_id, b.device_id, "the device and its data were kept");
    assert_eq!(lane2.held_sessions(NODE_ID), dir.node.sessions());
}

#[test]
fn an_offer_without_our_session_means_the_node_restarted() {
    let (dir, lane) = setup();
    let d = dref(&lane.boot(&req(None)).unwrap());
    let mut facts = dir.node.facts();
    lane.on_offer(NODE_ID, Some(&facts));
    assert!(lane.alive(&d), "still listed");
    facts.sessions.clear();
    lane.on_offer(NODE_ID, Some(&facts));
    assert!(!lane.alive(&d));
    assert!(lane.tree(&d).unwrap_err().message().contains("rig1-mac restarted"));
}

#[test]
fn the_node_refuses_what_its_policy_forbids_over_the_stream() {
    let (dir, lane) = setup();
    let b = lane.boot(&req(None)).unwrap();
    let conn = lane.conn_of(&key()).unwrap();
    let set = dir.node.paths().set.clone();
    let sim = |rest: &[&str]| {
        let mut v = vec!["simctl".to_string(), "--set".into(), set.clone()];
        v.extend(rest.iter().map(|s| s.to_string()));
        v
    };
    let refused = |r: crate::sim::ios::simctl::SimResult<_>| {
        let e = r.map(|_: crate::sim::ios::simctl::Output| ()).unwrap_err();
        assert_eq!(e.code, "node_refused", "{}", e.message);
    };
    refused(conn.run("xcrun", &["clang".into(), "-v".into()], Duration::from_secs(5)));
    refused(conn.run("xcrun", &sim(&["spawn", &b.device_id, "sh", "-c", "id"]), Duration::from_secs(5)));
    refused(conn.run("xcrun", &sim(&["openurl", &b.device_id, "file:///var/k2node/"]), Duration::from_secs(5)));
    refused(conn.run("xcrun", &sim(&["install", &b.device_id, "/var/k2node/state/x.app"]), Duration::from_secs(5)));
    refused(conn.run("/bin/sh", &["-c".into(), "id".into()], Duration::from_secs(5)));
    let mut other_set = sim(&["list", "-j", "devices"]);
    other_set[2] = "/var/k2sim/sets/1".into();
    refused(conn.run("xcrun", &other_set, Duration::from_secs(5)));
    let e = conn.disk(k2_node_proto::sim::ctl::DiskOp::Read, "/var/k2node/state/node-key.pem").unwrap_err();
    assert_eq!(e.code, "node_refused");
    // A helper for a device that isn't in this server's set.
    let e = conn.start_helper("AD6DB1AF-E89F-48D1-A6D7-DDEF9C68B1EB").err().expect("refused");
    assert_eq!(e.code, "node_refused");
    // The helper's `image` outside the workspace's folder.
    let h = conn.start_helper(&b.device_id).unwrap();
    let e = crate::sim::ios::helper::HelperIo::call(&*h, "image", json!({"path": "/var/k2node/state/x.png"}), Duration::from_secs(5)).unwrap_err();
    assert_eq!(e.code, "node_refused", "{}", e.message);
    // And every refusal maps to the lane's code.
    assert_eq!(crate::sim::ios::lane::lane_err(e).code(), "sim_node_refused");
}

#[test]
fn open_url_refuses_file_urls_and_this_servers_own_addresses() {
    let (dir, lane) = setup();
    let d = dref(&lane.boot(&req(None)).unwrap());
    for (url, code) in [
        ("file:///etc/passwd", "sim_invalid"),
        ("FILE:/Users/patlee", "sim_invalid"),
        ("http://localhost:3000", "sim_url_unreachable"),
        ("http://127.0.0.1:8081", "sim_url_unreachable"),
        ("http://192.168.1.20/", "sim_url_unreachable"),
        ("http://10.0.0.5", "sim_url_unreachable"),
        ("http://[::1]:5173", "sim_url_unreachable"),
        ("http://box.local", "sim_url_unreachable"),
    ] {
        let e = lane.act(&d, &Action::OpenUrl { url: url.into() }).unwrap_err();
        assert_eq!(e.code(), code, "{url}: {}", e.message());
    }
    let e = lane.act(&d, &Action::OpenUrl { url: "http://localhost:3000".into() }).unwrap_err();
    assert!(e.message().contains("rig1-mac") && e.message().contains("k2 app inspect"), "{}", e.message());
    for ok in ["https://web.app.k2.dev", "http://example.com", "myapp://open"] {
        lane.act(&d, &Action::OpenUrl { url: ok.into() }).unwrap();
    }
    assert_eq!(dir.node.world.lock().unwrap().opened.len(), 3);
}

#[test]
fn an_app_with_a_link_out_of_the_bundle_is_refused_before_upload() {
    let (dir, lane) = setup();
    let d = dref(&lane.boot(&req(None)).unwrap());
    let src = tmpdir("linky");
    let secret = src.join("state.db");
    std::fs::write(&secret, CANARY).unwrap();
    let app = fake_node::fixture_app(&src);
    std::os::unix::fs::symlink(&secret, app.join("db")).unwrap();
    let e = lane.install_app(&d, &app).unwrap_err();
    assert_eq!(e.code(), "sim_invalid");
    assert!(e.message().contains("links outside the app"), "{}", e.message());
    assert!(dir.node.world.lock().unwrap().installed.is_empty());
    assert!(!fake_node::relay_carries(&dir.node, CANARY.as_bytes()));
    assert!(!dir.node.log.lock().unwrap().iter().any(|l| l == "upload"), "nothing was sent");
    // A hostile archive sent straight to the node is refused there.
    let conn = lane.conn_of(&key()).unwrap();
    let mut b = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_old();
    h.as_old_mut().name[..17].copy_from_slice(b"A.app/../../evil\0");
    h.set_size(1);
    h.set_cksum();
    b.append(&h, &b"x"[..]).unwrap();
    let e = conn.upload(&b.into_inner().unwrap()).unwrap_err();
    assert_eq!(e.code, "bad_app");
    assert!(!dir.node.local("/var/k2sim/evil").exists());
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn node_selection_failures_teach_with_the_right_codes() {
    let wsreq = req(None);
    // No grant.
    let dir = FakeDirectory::new();
    let lane = Arc::new(IosNodeLane::new(dir.clone()).with_timeouts(Duration::from_secs(5), Duration::from_secs(5)));
    dir.node.attach_lane(&lane);
    let e = lane.boot(&wsreq).unwrap_err();
    assert_eq!((e.code(), e.http_status()), ("sim_node_not_granted", 403));
    assert!(e.message().contains("k2 compute grant rig1-mac"), "{}", e.message());
    assert_eq!(crate::sim::manager::SimError::from(e).exit, 3);
    dir.grant(WS_ID);
    // Held (a foreign lock, as the offer says).
    *dir.node.hold.lock().unwrap() = Some("holding for Sew CI".into());
    let e = lane.boot(&wsreq).unwrap_err();
    assert_eq!(e.code(), "sim_node_busy");
    assert!(e.message().contains("Sew CI"), "{}", e.message());
    assert_eq!(crate::sim::manager::SimError::from(e).exit, 2);
    *dir.node.hold.lock().unwrap() = None;
    // Offline.
    *dir.node.online.lock().unwrap() = false;
    assert!(lane.boot(&wsreq).unwrap_err().message().contains("offline"));
    *dir.node.online.lock().unwrap() = true;
    // Not ready: the owner's step, on that node.
    *dir.node.ready.lock().unwrap() = false;
    let e = lane.boot(&wsreq).unwrap_err();
    assert_eq!(e.code(), "sim_needs_setup");
    assert!(e.message().starts_with("On rig1-mac: install an iOS Simulator runtime"), "{}", e.message());
    let p = lane.probe();
    assert!(p.usable && !p.ready && !p.needs.is_empty(), "{p:?}");
    *dir.node.ready.lock().unwrap() = true;
    // Outdated helper: the line fits the node's admin level (IR-13).
    *dir.node.helper_protocol.lock().unwrap() = u64::from(crate::sim::ios::helper::OLDEST_PROTOCOL) - 1;
    let e = lane.boot(&wsreq).unwrap_err();
    assert_eq!(e.code(), "sim_node_outdated");
    assert!(e.message().contains("sudo k2-node admin level policy"), "remote admin is off: {}", e.message());
    *dir.node.helper_protocol.lock().unwrap() = psim::HELPER_PROTOCOL;
    // The node itself refuses (its foreign lock, unknown to the offer yet).
    dir.node.admission.lock().unwrap().foreign_lock = Some("Sew CI".into());
    let e = lane.boot(&wsreq).unwrap_err();
    assert_eq!(e.code(), "sim_node_busy");
    assert!(e.message().contains("holding for Sew CI"), "{}", e.message());
    dir.node.admission.lock().unwrap().foreign_lock = None;
    // A named device the node doesn't have.
    let e = lane.boot(&req(Some("rig1-mac:Nokia 3310"))).unwrap_err();
    assert_eq!(e.code(), "sim_invalid");
    // A named node that runs no iPhones.
    assert_eq!(lane.boot(&req(Some("rig2-mac:iPhone 17"))).unwrap_err().code(), "sim_lane_unavailable");
}

/// The grant rule matches `k2 compute run`: the owner and admins may boot
/// on any active Mac node without a grant; everyone else needs one.
#[test]
fn the_owner_and_admins_need_no_grant() {
    let dir = FakeDirectory::new();
    let lane = Arc::new(IosNodeLane::new(dir.clone()).with_timeouts(Duration::from_secs(10), Duration::from_secs(120)));
    dir.node.attach_lane(&lane);
    assert_eq!(lane.boot(&req(None)).unwrap_err().code(), "sim_node_not_granted");
    assert!(dir.node.sessions().is_empty());
    let b = lane.boot(&BootRequest { grant_exempt: true, ..req(None) }).expect("an owner or admin boots without a grant");
    assert_eq!(b.profile.name, "iPhone 17 (on rig1-mac)");
    assert_eq!(dir.node.sessions().len(), 1);
}

/// A lease follows compute placement: a workspace's node list (`k2 compute
/// allow-nodes`) keeps its iPhones off every other Mac, the owner's too
/// (`node_not_allowed`, exit 3, before anything is leased), and a
/// pinned-only Mac takes an iPhone only when the device names it.
#[test]
fn the_node_list_and_pinned_only_hold_for_a_lease() {
    let (dir, lane) = setup();
    dir.disallow(WS_ID);
    for r in [req(None), BootRequest { grant_exempt: true, ..req(None) }] {
        let e = lane.boot(&r).unwrap_err();
        assert_eq!((e.code(), e.http_status()), ("node_not_allowed", 403), "{}", e.message());
        assert!(matches!(e, LaneError::Node { reason: NodeReason::NotAllowed, .. }));
        assert_eq!(NodeReason::NotAllowed.exit(), 3, "a person changes the list");
        assert!(e.message().contains("k2 compute allow-nodes --add rig1-mac"), "{}", e.message());
    }
    assert!(dir.node.sessions().is_empty(), "nothing leased off the list");
    dir.allow_again(WS_ID);

    *dir.node.pinned_only.lock().unwrap() = true;
    let e = lane.boot(&req(None)).unwrap_err();
    assert_eq!(e.code(), "sim_lane_unavailable", "automatic placement never picks a pinned-only Mac: {}", e.message());
    assert!(dir.node.sessions().is_empty());
    let b = lane.boot(&req(Some(&format!("{NODE_NAME}:{TYPE_17_PRO}")))).expect("a device that names the Mac boots there");
    assert_eq!(b.profile.name, "iPhone 17 Pro (on rig1-mac)");
    assert_eq!(dir.node.sessions().len(), 1);
}

#[test]
fn one_iphone_per_node_and_a_pick_names_its_device() {
    let (dir, lane) = setup();
    dir.grant("proj-other");
    let b = lane.boot(&req(Some(&format!("{NODE_NAME}:{TYPE_17_PRO}")))).unwrap();
    assert_eq!(b.profile.name, "iPhone 17 Pro (on rig1-mac)");
    let mut other = req(None);
    other.workspace_id = "proj-other".into();
    other.workspace_key = crate::sim::workspace_key("proj-other");
    let e = lane.boot(&other).unwrap_err();
    assert_eq!(e.code(), "sim_node_busy");
    assert!(e.message().contains("running another iPhone"), "{}", e.message());
}

/// A server with no Mac node: the lane is registered but not usable, and
/// says what to do (IR-11: one code, `sim_lane_unavailable`).
struct NoNodes;
impl NodeDirectory for NoNodes {
    fn nodes(&self) -> Vec<NodeInfo> {
        vec![NodeInfo { id: "n2".into(), name: "rig9-vps".into(), online: true, protocol: 7, features: vec![], mac: false, sim: None, hold: None, pinned_only: false }]
    }
    fn granted(&self, _: &str, _: &str) -> bool {
        true
    }
    fn allowed(&self, _: &str, _: &str) -> bool {
        true
    }
    fn send(&self, _: &str, _: Frame) -> bool {
        panic!("nothing is sent with no Mac node")
    }
    fn node_spki(&self, _: &str) -> Option<Vec<u8>> {
        None
    }
    fn node_by_fp(&self, _: &str) -> Option<String> {
        None
    }
    fn controller_key(&self) -> Result<k2_node_proto::crypto::SigningKey, String> {
        Err("none".into())
    }
}

#[test]
fn no_mac_node_is_unusable_and_teaches() {
    let lane = IosNodeLane::new(Arc::new(NoNodes));
    let p = lane.probe();
    assert!(!p.usable && !p.ready);
    let detail = p.detail.as_deref().unwrap();
    assert!(detail.contains("as a compute node") && detail.contains("`k2 compute node add <name>`") && detail.contains("`k2 study compute`") && detail.contains("`sudo k2-node setup --sim`"), "{p:?}");
    let e = lane.boot(&req(None)).unwrap_err();
    assert_eq!((e.code(), e.http_status()), ("sim_lane_unavailable", 409));
    assert_eq!(crate::sim::manager::SimError::from(e).exit, 3);
    assert_eq!(lane.profiles().unwrap_err().code(), "sim_lane_unavailable");
}

/// Old peers never see a sim frame: a node offers iPhones only at
/// protocol 7 with the feature, on a Mac, with facts.
#[test]
fn only_a_protocol_7_mac_offering_sim_ios_counts() {
    let dir = FakeDirectory::new();
    let ok = dir.node.info();
    assert!(ok.offers_sim());
    assert!(!NodeInfo { protocol: 6, ..ok.clone() }.offers_sim());
    assert!(!NodeInfo { features: vec![], ..ok.clone() }.offers_sim());
    assert!(!NodeInfo { mac: false, ..ok.clone() }.offers_sim());
    assert!(!NodeInfo { sim: None, ..ok }.offers_sim());
}

#[test]
fn a_stream_hello_for_someone_elses_session_is_refused() {
    let (dir, lane) = setup();
    lane.boot(&req(None)).unwrap();
    let session = dir.node.sessions()[0].clone();
    // Another node's key claiming this session.
    let stranger = k2_node_proto::crypto::SigningKey::generate().unwrap();
    let (_, hello) = k2_node_proto::sim::stream::NodeHandshake::start(&stranger, &session).unwrap();
    assert!(lane.stream_hello(&hello).is_err());
    // A session nobody opened.
    let (_, hello) = k2_node_proto::sim::stream::NodeHandshake::start(&dir.node.key, &"f".repeat(32)).unwrap();
    assert!(lane.stream_hello(&hello).is_err());
}
