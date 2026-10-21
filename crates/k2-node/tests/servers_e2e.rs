//! One node, many servers (prd-compute-shared-queue-v1 S1, tier 1): a real
//! `Node` with two loopback fake servers built from `k2-node-proto` only.
//! Temp dirs, fake servers, no real node, user or daemon.
//!
//! - enrol by code binding and SAS, one session per server, each offer and
//!   snapshot only its own (V53, S1, S2, S4);
//! - owner-only frames dropped for another server (V52, P4);
//! - a server's `Revoked` ends only that server (V51, P6);
//! - the node owner's pause, end date and revoke (§4.3);
//! - a 0.46.2 server refused after its SAS (`server_too_old`, V79);
//! - a node without its wall serves only the owner (V59);
//! - no `servers.toml`: solo, today's offer (plus `servers` to protocol 3).
//!
//! Jobs run behind the Seatbelt wall, so those parts are macOS (the minis);
//! the rest runs anywhere.

mod common;

use std::time::Duration;

use common::*;
use k2_node::admin::servers;
use k2_node::identity::{Pin, TenantFlags};
use k2_node::wall::Wall;
use k2_node_proto::crypto::SigningKey;
use k2_node_proto::frames::{self, Frame, HandshakeFrame, JobState};
use k2_node_proto::pairing::{self, EnrollString};
use k2_node_proto::share::{codes, Network, Role, ServerEntry, ServerStatus, ServersFile, ShareLimits, ShareState, Trust};

/// The node's wall, and an executor socket path no real executor listens
/// on (a test node never asks this machine's executor anything).
fn walled(w: Wall) -> impl FnOnce(&mut k2_node::node::NodeOptions) {
    move |o| {
        o.wall = Some(w);
        o.admin_socket = k2_node::util::temp_dir("no-executor").join("none.sock");
    }
}

fn ctx(n: &TestNode) -> servers::Ctx {
    servers::Ctx {
        config_dir: n.layout.config.clone(),
        state_dir: n.dir.join("admin-state"),
        home: n.layout.home.clone(),
        now: k2_node::util::now(),
        macos: cfg!(target_os = "macos"),
        walled: true,
        tenant_wall: false,
        pinned_owner: None,
        node_user: "nobody".into(),
        by: "local:test".into(),
    }
}

async fn eventually<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    for _ in 0..400 {
        if let Some(v) = f() {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

fn status(n: &TestNode) -> k2_node::status::Status {
    k2_node::status::read(&n.layout.status()).unwrap()
}

/// `servers.toml` with the owner (from the node's pin) and one more line.
fn write_shared(n: &TestNode, slot: u8, fp_prefix: &str, label: &str, state: ShareState, expires_at: Option<i64>) {
    let owner = n.node.owner().pin.lock().unwrap().clone().unwrap();
    let now = k2_node::util::now();
    let mut f = ServersFile::new(&owner.controller_fp, "owner.k2.dev", ShareLimits::owner(2, 8, 16, 100, Network::On), 8, now, "local:test");
    let mut e: ServerEntry = f.servers[0].clone();
    e.slot = slot;
    e.label = label.into();
    e.fp_prefix = fp_prefix.into();
    e.trust = Trust::Own;
    e.state = state;
    e.set_limits(&ShareLimits::new_server(Trust::Own, false));
    e.expires_at = expires_at;
    e.uid = Some(61001);
    f.servers.push(e);
    servers::write_file(&n.layout.config, &f).unwrap();
}

/// A second server already enrolled and confirmed (its pin on disk, the
/// fake knows the node as active).
fn pin_second(n: &TestNode, b: &Fake, slot: u8, node_id: &str) {
    let key = k2_node::identity::load_or_create_key(&n.layout.key()).unwrap();
    b.register_active(&key, node_id, "mini-1");
    let t = n.layout.tenant(slot);
    t.ensure_state().unwrap();
    k2_node::identity::write_tenant_pin(&t.pin(), &b.pin_for(node_id, "mini-1"), &TenantFlags::default()).unwrap();
}

fn prefix(k: &SigningKey) -> String {
    k.fingerprint()[..16].to_string()
}

fn offer_of(f: &[Frame]) -> frames::Offer {
    f.iter()
        .rev()
        .find_map(|x| match x {
            Frame::Offer(o) => Some(o.clone()),
            _ => None,
        })
        .expect("an offer")
}

fn has(o: &frames::Offer, feature: &str) -> bool {
    o.features.iter().any(|f| f == feature)
}

/// A node with no `servers.toml` is solo: the owner's offer has no share,
/// and to a protocol-3 server only the `servers` feature is new.
#[tokio::test(flavor = "multi_thread")]
async fn solo_is_today_plus_the_servers_feature() {
    let mut a = Fake::start().await;
    let n = start_node(&a, "srv-solo", "", walled(Wall::Seatbelt)).await;
    let mut ca = a.next_conn().await;
    let o = offer_of(&ca.until(|f| matches!(f, Frame::Offer(_))).await);
    assert!(o.share.is_none(), "nobody shares this node");
    for f in [k2_node_proto::admin::FEATURE_ADMIN, k2_node_proto::admin::FEATURE_SELF_UPDATE, k2_node_proto::admin::FEATURE_HEALTH, k2_node_proto::share::FEATURE_SERVERS] {
        assert!(has(&o, f), "{f} in {:?}", o.features);
    }
    assert!(n.node.tenants().len() == 1 && status(&n).servers.is_empty() && status(&n).servers_error.is_none());
    assert_eq!(n.node.owner().protocol(), frames::PROTOCOL);
}

/// §4.1 and V53: the node enrols with a second server by the code's
/// binding, the SAS shows on the node, and once its owner confirms it the
/// second session runs beside the owner's, seeing only its own share.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_server_enrols_by_sas_and_sees_only_its_own() {
    let mut a = Fake::start().await;
    let mut b = Fake::start().await;
    let n = start_node(&a, "srv-enrol", "", walled(Wall::Seatbelt)).await;
    let mut ca = a.next_conn().await;
    ready(&mut ca).await;

    // B mints a code; the node owner adds B with it.
    let code = pairing::new_code().unwrap();
    let norm = pairing::normalize_code(&code).unwrap();
    b.codes.lock().unwrap().push(norm.clone());
    let es = EnrollString::new(&code, &b.key.fingerprint()).unwrap();
    let args = serde_json::json!({
        "url": b.base, "code_binding": pairing::code_binding(&norm), "fp_prefix": es.controller_fp_prefix, "trust": "own",
    });
    let r = servers::run(&ctx(&n), "servers.add", &args).unwrap();
    assert_eq!(r["slot"], 1);
    // The audit never sees the binding.
    assert_eq!(k2_node::admin::exec::audit_args("servers.add", &args)["code_binding"], "<redacted>");

    // The node enrols itself; the SAS it shows is the one B computed.
    let line = eventually("the SAS on the node", || status(&n).servers.into_iter().find(|l| l.slot == 1 && l.sas.is_some())).await;
    assert_eq!(line.sas, b.last_sas.lock().unwrap().clone(), "both sides show the same SAS");
    assert_eq!(line.state, "pending");
    eventually("pending at B", || (n.node.tenant(1).map(|t| t.link.lock().unwrap().state.clone()).as_deref() == Some("pending")).then_some(())).await;
    let t1 = n.node.tenant(1).unwrap();
    assert_eq!(t1.job_gate(n.node.now(), true), Some(codes::SERVER_NOT_APPROVED), "no jobs before the SAS");

    // B's owner confirms the SAS: the second session starts.
    let key = n.node.key.fingerprint();
    b.nodes.lock().unwrap().get_mut(&key).unwrap().state = NodeState::Active;
    let mut cb = b.next_conn().await;
    let first = cb.until(|f| matches!(f, Frame::Offer(_))).await;
    assert!(matches!(first[0], Frame::JobsSnapshot(ref s) if s.jobs.is_empty()), "B's snapshot is its own (empty)");
    assert!(offer_of(&first).share.is_some(), "B speaks protocol 3: its share view");
    // The servers loop counts B in on its next pass (maybe already).
    let shared = |o: &frames::Offer| o.share.as_ref().is_some_and(|s| s.shared);
    let ob = match offer_of(&first) {
        o if shared(&o) => o,
        _ => offer_of(&cb.until(|f| matches!(f, Frame::Offer(o) if shared(o))).await),
    };
    let share = ob.share.clone().unwrap();
    assert_eq!((share.role, share.slot, share.state, share.shared, share.servers), (Role::Tenant, 1, ServerStatus::Active, true, 2));
    assert_eq!(share.owner_label, Some(servers::label_of(&a.base)), "shared by the owner");
    for f in [k2_node_proto::admin::FEATURE_ADMIN, k2_node_proto::admin::FEATURE_SELF_UPDATE, k2_node_proto::admin::FEATURE_HEALTH] {
        assert!(!has(&ob, f), "B is offered no {f} (V52): {:?}", ob.features);
    }
    assert!(has(&ob, k2_node_proto::share::FEATURE_SERVERS) && has(&ob, frames::FEATURE_JOB_WALL));
    assert!(ob.control_by.is_none(), "no person's name to another server");
    assert_ne!(ob.ledger_id, n.node.offer(&n.node.owner()).ledger_id, "per-server ledgers");
    assert!(t1.pin.lock().unwrap().as_ref().unwrap().confirmed);

    // The owner now sees a shared node.
    let oa = offer_of(&ca.until(|f| matches!(f, Frame::Offer(o) if o.share.is_some())).await);
    let sa = oa.share.clone().unwrap();
    assert_eq!((sa.role, sa.slot, sa.servers, sa.shared, sa.owner_label), (Role::Owner, 0, 2, true, None));
    assert!(has(&oa, k2_node_proto::admin::FEATURE_ADMIN), "the owner keeps admin");

    // V53: B's offer lists none of the owner's warm slots (or their SHAs).
    let pk = n.layout.cache().join("owner-project");
    std::fs::create_dir_all(&pk).unwrap();
    std::fs::write(pk.join("slots.json"), r#"{"1":"0123abcd"}"#).unwrap();
    assert!(n.node.offer(&n.node.owner()).slots.iter().any(|s| s.project_key == "owner-project"));
    assert!(n.node.offer(&t1).slots.is_empty(), "B sees no owner slot");

    // S2: B asking about a job only the owner has gets `unknown`.
    cb.send(Frame::Cancel(frames::Cancel { job_id: "only-a".into(), generation: 1, reason: "cancelled".into() }));
    let s = cb.until(|f| matches!(f, Frame::State(s) if s.job_id == "only-a")).await;
    assert!(matches!(s.last().unwrap(), Frame::State(s) if s.state == JobState::Unknown));

    // The status file lists B online; servers.list (the owner's read verb)
    // says the same.
    let line = eventually("B online", || status(&n).servers.into_iter().find(|l| l.slot == 1 && l.link == "online")).await;
    assert_eq!((line.state.as_str(), line.sas.as_deref()), ("active", None));
    let list = servers::node_list(&n.node);
    assert_eq!(list["servers"][1]["state"], "active", "{list}");
    assert_eq!(list["servers"][0]["role"], "owner");
}

/// V52 / P4: another server's admin, keys and update frames are dropped
/// before they touch state (the owner's transfer survives), answered
/// `not_node_owner` once per session.
#[tokio::test(flavor = "multi_thread")]
async fn another_server_gets_no_admin_or_updates() {
    let mut a = Fake::start().await;
    let mut b = Fake::start().await;
    let n = start_node(&a, "srv-v52", "", walled(Wall::Seatbelt)).await;
    let mut ca = a.next_conn().await;
    ready(&mut ca).await;
    pin_second(&n, &b, 1, "node-b");
    write_shared(&n, 1, &prefix(&b.key), "b.k2.dev", ShareState::Active, None);
    let mut cb = b.next_conn().await;
    ready(&mut cb).await;

    // The owner starts sending a build.
    let cmd = k2_node_proto::admin::SignedCommand { body_b64: "e30".into(), sig_c: "x".into(), sig_o: None, plan_b64: None };
    let offer = |id: &str| frames::UpdateOffer {
        transfer_id: id.into(),
        version: "9.9.9".into(),
        protocol: frames::PROTOCOL,
        os: k2_node::sysinfo::os_label().into(),
        arch: std::env::consts::ARCH.into(),
        sha256: "0".repeat(64),
        size: 10,
        sig: "s".into(),
        cmd: cmd.clone(),
    };
    ca.send(Frame::UpdateOffer(offer("owner-build")));
    eventually("the owner's transfer", || n.node.admin_updates.transfer_ids().contains(&"owner-build".to_string()).then_some(())).await;

    // B tries admin, then an update of its own.
    cb.send(Frame::Admin(frames::AdminFrame { cmd: cmd.clone() }));
    let r = cb.until(|f| matches!(f, Frame::AdminResult(_))).await;
    match r.last().unwrap() {
        Frame::AdminResult(r) => assert_eq!(r.code.as_deref(), Some(codes::NOT_NODE_OWNER)),
        _ => unreachable!(),
    }
    cb.send(Frame::UpdateOffer(offer("b-build")));
    cb.send(Frame::UpdateChunk(frames::UpdateChunk { transfer_id: "owner-build".into(), seq: 0, data: "AAAA".into(), last: true }));
    cb.send(Frame::Cancel(frames::Cancel { job_id: "probe".into(), generation: 1, reason: "cancelled".into() }));
    // Frames are handled in order: once the cancel is answered, the update
    // frames before it were already dropped (and told only once).
    let seen = cb.until(|f| matches!(f, Frame::State(s) if s.job_id == "probe")).await;
    assert!(!seen.iter().any(|f| matches!(f, Frame::UpdateState(_) | Frame::AdminResult(_))), "not_node_owner is said once: {seen:?}");
    assert!(!seen.iter().any(|f| matches!(f, Frame::Health(_))), "no health to B");
    assert_eq!(n.node.admin_updates.transfer_ids(), vec!["owner-build".to_string()], "B neither replaced nor fed the owner's transfer");
}

/// V51 / P6: B's `Revoked` ends only B: B's running job stops, the
/// owner's same-named job keeps running, `controller.json` is untouched.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
async fn a_tenants_revoked_ends_only_that_tenant() {
    let mut a = Fake::start().await;
    let mut b = Fake::start().await;
    let n = start_node(&a, "srv-v51", "", walled(Wall::Seatbelt)).await;
    let mut ca = a.next_conn().await;
    ready(&mut ca).await;
    pin_second(&n, &b, 1, "node-b");
    write_shared(&n, 1, &prefix(&b.key), "b.k2.dev", ShareState::Active, None);
    let mut cb = b.next_conn().await;
    ready(&mut cb).await;
    let owner_pin = std::fs::read(n.layout.pin()).unwrap();

    ca.send(Frame::Assign(assign(plan("j", &["/bin/sh", "-c", "echo owner; sleep 30"]), 1)));
    ca.until(|f| matches!(f, Frame::State(s) if s.job_id == "j" && s.state == JobState::Running)).await;
    let mut pb = plan("j", &["/bin/sh", "-c", "echo tenant; sleep 30"]);
    pb.node_id = "node-b".into();
    cb.send(Frame::Assign(assign(pb, 1)));
    cb.until(|f| matches!(f, Frame::State(s) if s.job_id == "j" && s.state == JobState::Running)).await;
    assert_eq!(n.node.running_by_tenant().len(), 2, "the same job id runs once per server (V48)");

    cb.send(Frame::Revoked(frames::Revoked { reason: "removed by b".into() }));
    assert!(cb.wait_closed(Duration::from_secs(10)).await, "B's session ends");
    let t1 = n.node.tenant(1).unwrap();
    let row = eventually("B's job to stop", || t1.ledger.lock().unwrap().get("j", 1).unwrap().filter(|r| r.state.is_terminal())).await;
    assert_eq!((row.state, row.reason.as_deref()), (JobState::Cancelled, Some("node_removed")));
    let a_row = n.node.owner().ledger.lock().unwrap().get("j", 1).unwrap().unwrap();
    assert!(!a_row.state.is_terminal(), "the owner's job keeps running: {:?}", a_row.state);
    assert_eq!(std::fs::read(n.layout.pin()).unwrap(), owner_pin, "controller.json unchanged");
    assert!(!n.node.owner().pin.lock().unwrap().as_ref().unwrap().revoked);
    let (pin, flags) = k2_node::identity::read_tenant_pin(&n.layout.tenant(1).pin()).unwrap().unwrap();
    assert!(pin.revoked && flags.revoked_by_server, "B's own pin says B left");
    eventually("B listed revoked", || status(&n).servers.into_iter().find(|l| l.slot == 1 && l.state == "revoked")).await;
    assert_ne!(status(&n).state, "revoked", "the node itself isn't");
    ca.send(Frame::Cancel(frames::Cancel { job_id: "j".into(), generation: 1, reason: "cancelled".into() }));
    run_to_receipt(&mut ca, "j").await;
}

/// §4.3: the node owner's pause leaves B's new jobs queued on B; resume
/// lets them run; revoke `--now` stops B's running job and closes B's
/// session with a last offer saying so. The owner is never touched.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
async fn pause_resume_and_revoke_now_act_on_that_server_only() {
    let mut a = Fake::start().await;
    let mut b = Fake::start().await;
    let n = start_node(&a, "srv-pause", "", walled(Wall::Seatbelt)).await;
    let mut ca = a.next_conn().await;
    ready(&mut ca).await;
    pin_second(&n, &b, 1, "node-b");
    write_shared(&n, 1, &prefix(&b.key), "b.k2.dev", ShareState::Active, None);
    let mut cb = b.next_conn().await;
    ready(&mut cb).await;
    let c = ctx(&n);
    let t1 = n.node.tenant(1).unwrap();
    let bplan = |job: &str, argv: &[&str]| {
        let mut p = plan(job, argv);
        p.node_id = "node-b".into();
        assign(p, 1)
    };

    servers::run(&c, "servers.pause", &serde_json::json!({"server": "b.k2.dev"})).unwrap();
    // B is told its share is paused.
    cb.until(|f| matches!(f, Frame::Offer(o) if o.share.as_ref().is_some_and(|s| s.state == ServerStatus::Paused))).await;
    assert_eq!(t1.status_at(n.node.now()), Some(ServerStatus::Paused));
    // A paused share still queues (§4.3): the job waits in the node queue,
    // blocked by the pause, and starts once the share resumes.
    cb.send(Frame::Assign(bplan("p1", &["/bin/echo", "x"])));
    let s = cb.until(|f| matches!(f, Frame::State(s) if s.job_id == "p1" && s.queue.is_some())).await;
    match s.last().unwrap() {
        Frame::State(s) => {
            assert_eq!((s.state, s.reason.as_deref()), (JobState::Queued, Some(codes::NODE_QUEUED)));
            assert_eq!(s.queue.as_ref().unwrap().blocked_by.as_deref(), Some(codes::SHARE_PAUSED), "{s:?}");
        }
        _ => unreachable!(),
    }
    assert_eq!(t1.ledger.lock().unwrap().get("p1", 1).unwrap().unwrap().state, JobState::Queued);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(n.node.running_by_tenant().get(&1).is_none(), "nothing of a paused share starts");

    servers::run(&c, "servers.resume", &serde_json::json!({"server": "slot:1"})).unwrap();
    eventually("the resume", || (t1.status_at(n.node.now()) == Some(ServerStatus::Active)).then_some(())).await;
    let r = run_to_receipt(&mut cb, "p1").await;
    assert_eq!(states_of(&r, "p1").last().unwrap().state, JobState::Done, "the queued job ran after the resume");
    cb.send(Frame::Assign(bplan("r1", &["/bin/sh", "-c", "echo b; sleep 30"])));
    cb.until(|f| matches!(f, Frame::State(s) if s.job_id == "r1" && s.state == JobState::Running)).await;
    ca.send(Frame::Assign(assign(plan("a1", &["/bin/sh", "-c", "echo a; sleep 30"]), 1)));
    ca.until(|f| matches!(f, Frame::State(s) if s.job_id == "a1" && s.state == JobState::Running)).await;

    let mut rc = c.clone();
    rc.now = k2_node::util::now();
    servers::run(&rc, "servers.revoke", &serde_json::json!({"server": "b.k2.dev", "now": true})).unwrap();
    let last = cb.until(|f| matches!(f, Frame::Offer(o) if o.share.as_ref().is_some_and(|s| s.state == ServerStatus::Revoked))).await;
    assert!(matches!(last.last(), Some(Frame::Offer(_))));
    assert!(cb.wait_closed(Duration::from_secs(10)).await, "B's session closes");
    let row = eventually("B's job to stop", || t1.ledger.lock().unwrap().get("r1", 1).unwrap().filter(|r| r.state.is_terminal())).await;
    assert_eq!((row.state, row.reason.as_deref()), (JobState::Cancelled, Some(codes::SHARE_REVOKED)));
    assert!(!n.node.owner().ledger.lock().unwrap().get("a1", 1).unwrap().unwrap().state.is_terminal(), "the owner's job runs on");
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(b.attaches.load(std::sync::atomic::Ordering::SeqCst), 1, "a revoked server is never dialled again");
    ca.send(Frame::Cancel(frames::Cancel { job_id: "a1".into(), generation: 1, reason: "cancelled".into() }));
    run_to_receipt(&mut ca, "a1").await;
}

/// §4.3, V64, P7 (node half): revoke `--wipe` stops the server's running
/// job, wipes its slot (pin, ledger, journals, root) and frees the slot;
/// the next server added there enrols afresh with a new uid and sees
/// nothing of the old one.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
async fn a_wiped_slot_is_reused_fresh() {
    let mut a = Fake::start().await;
    let mut b = Fake::start().await;
    let n = start_node(&a, "srv-wipe", "", walled(Wall::Seatbelt)).await;
    let mut ca = a.next_conn().await;
    ready(&mut ca).await;
    let c = ctx(&n);
    // B is added and confirmed through the real path.
    let add = |f: &Fake, label_url: &str| {
        let code = pairing::new_code().unwrap();
        let norm = pairing::normalize_code(&code).unwrap();
        f.codes.lock().unwrap().push(norm.clone());
        let es = EnrollString::new(&code, &f.key.fingerprint()).unwrap();
        serde_json::json!({"url": label_url, "code_binding": pairing::code_binding(&norm), "fp_prefix": es.controller_fp_prefix, "trust": "own"})
    };
    let r = servers::run(&c, "servers.add", &add(&b, &b.base)).unwrap();
    let uid_b = r["uid"].as_u64().unwrap();
    eventually("B's SAS", || status(&n).servers.into_iter().find(|l| l.slot == 1 && l.sas.is_some())).await;
    let key = n.node.key.fingerprint();
    b.nodes.lock().unwrap().get_mut(&key).unwrap().state = NodeState::Active;
    let mut cb = b.next_conn().await;
    ready(&mut cb).await;
    let node_b = n.node.tenant(1).unwrap().node_id();
    let mut p = plan("w1", &["/bin/sh", "-c", "echo b; sleep 30"]);
    p.node_id = node_b;
    cb.send(Frame::Assign(assign(p, 1)));
    cb.until(|f| matches!(f, Frame::State(s) if s.job_id == "w1" && s.state == JobState::Running)).await;
    let slot_state = n.layout.tenant(1).state();
    assert!(slot_state.join("ledger.sqlite").exists());

    // Revoke and wipe at once.
    let mut rc = ctx(&n);
    rc.now = k2_node::util::now();
    let r = servers::run(&rc, "servers.revoke", &serde_json::json!({"server": "slot:1", "wipe": true})).unwrap();
    assert_eq!(r["wiped"], serde_json::json!([1]), "{r}");
    assert!(cb.wait_closed(Duration::from_secs(10)).await, "B's session ends");
    eventually("B's slot forgotten", || n.node.tenant(1).is_none().then_some(())).await;
    assert!(n.node.running_by_tenant().get(&1).is_none(), "B's job stopped");
    assert!(!n.layout.tenant(1).pin().exists());

    // C takes slot 1: a fresh enrol, a new uid, an empty ledger.
    let mut cc_fake = Fake::start().await;
    let r = servers::run(&ctx(&n), "servers.add", &add(&cc_fake, &cc_fake.base)).unwrap();
    assert_eq!(r["slot"], 1);
    assert!(r["uid"].as_u64().unwrap() > uid_b, "a reused slot never reuses a uid: {r}");
    eventually("C's SAS", || status(&n).servers.into_iter().find(|l| l.slot == 1 && l.sas.is_some())).await;
    cc_fake.nodes.lock().unwrap().get_mut(&key).unwrap().state = NodeState::Active;
    let mut cc = cc_fake.next_conn().await;
    let first = cc.until(|f| matches!(f, Frame::Offer(_))).await;
    assert!(matches!(first[0], Frame::JobsSnapshot(ref s) if s.jobs.is_empty()), "C sees nothing of B: {:?}", first[0]);
    assert!(n.node.tenant(1).unwrap().ledger.lock().unwrap().get("w1", 1).unwrap().is_none());
}

/// §4.3: past its end date a server with nothing running isn't served
/// (judged by time, nothing written); a server whose line is live is.
#[tokio::test(flavor = "multi_thread")]
async fn an_expired_share_is_not_served() {
    let mut a = Fake::start().await;
    let b = Fake::start().await;
    let n = start_node(&a, "srv-exp", "", walled(Wall::Seatbelt)).await;
    let mut ca = a.next_conn().await;
    ready(&mut ca).await;
    pin_second(&n, &b, 1, "node-b");
    write_shared(&n, 1, &prefix(&b.key), "b.k2.dev", ShareState::Active, Some(k2_node::util::now() - 1));
    let line = eventually("B listed", || status(&n).servers.into_iter().find(|l| l.slot == 1)).await;
    assert_eq!(line.state, "expired");
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(b.attaches.load(std::sync::atomic::Ordering::SeqCst), 0, "an expired share with no jobs isn't dialled");
    let t1 = n.node.tenant(1).unwrap();
    assert_eq!(t1.job_gate(n.node.now(), true), Some(codes::SHARE_EXPIRED));
    assert!(!n.node.offer(&n.node.owner()).share.is_some_and(|s| s.shared), "one active server: not shared");
}

/// V59: a node without its job wall serves only its owner; another
/// server's jobs wait `tenant_wall_missing`.
#[tokio::test(flavor = "multi_thread")]
async fn without_the_wall_only_the_owner_runs_jobs() {
    let mut a = Fake::start().await;
    let mut b = Fake::start().await;
    let n = start_node(&a, "srv-v59", "", walled(Wall::None { why: "test".into() })).await;
    let mut ca = a.next_conn().await;
    ready(&mut ca).await;
    pin_second(&n, &b, 1, "node-b");
    write_shared(&n, 1, &prefix(&b.key), "b.k2.dev", ShareState::Active, None);
    let mut cb = b.next_conn().await;
    ready(&mut cb).await;
    let mut p = plan("w1", &["/bin/echo", "x"]);
    p.node_id = "node-b".into();
    cb.send(Frame::Assign(assign(p, 1)));
    let s = cb.until(|f| matches!(f, Frame::State(s) if s.job_id == "w1")).await;
    assert!(matches!(s.last().unwrap(), Frame::State(s) if s.state == JobState::Queued && s.reason.as_deref() == Some(codes::TENANT_WALL_MISSING)), "{s:?}");
    // The owner still runs.
    ca.send(Frame::Assign(assign(plan("o1", &["/bin/echo", "owner"]), 1)));
    let fr = run_to_receipt(&mut ca, "o1").await;
    assert_eq!(states_of(&fr, "o1").last().unwrap().state, JobState::Done);
    // And an add is refused on an unwalled node.
    let mut c = ctx(&n);
    c.walled = servers::node_walled(&n.layout, cfg!(target_os = "macos"));
    assert!(!c.walled, "status.json says no wall");
    let e = servers::run(&c, "servers.add", &serde_json::json!({"url": "https://c.k2.dev", "code_binding": "x", "fp_prefix": "0123456789abcdef", "trust": "own"})).unwrap_err();
    assert_eq!(e.code, codes::TENANT_NEEDS_JOB_WALL);
}

/// §14: a broken `servers.toml` never fails open: the node keeps the last
/// good copy (B stays served) and says what's wrong.
#[tokio::test(flavor = "multi_thread")]
async fn a_broken_servers_file_keeps_the_last_good_copy() {
    let mut a = Fake::start().await;
    let mut b = Fake::start().await;
    let n = start_node(&a, "srv-broken", "", walled(Wall::Seatbelt)).await;
    let mut ca = a.next_conn().await;
    ready(&mut ca).await;
    pin_second(&n, &b, 1, "node-b");
    write_shared(&n, 1, &prefix(&b.key), "b.k2.dev", ShareState::Active, None);
    let mut cb = b.next_conn().await;
    ready(&mut cb).await;
    std::fs::write(n.layout.config.join("servers.toml"), "version = 1\nowner = \"nope\"\n").unwrap();
    let st = eventually("the error", || status(&n).servers_error.is_some().then(|| status(&n))).await;
    assert!(st.servers_error.unwrap().contains("last good"));
    let t1 = n.node.tenant(1).unwrap();
    assert_eq!(t1.status_at(n.node.now()), Some(ServerStatus::Active));
    assert!(*t1.serve.borrow(), "B is still served");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!cb.is_closed.load(std::sync::atomic::Ordering::SeqCst));
}

/// V79: a 0.46.2 server enrols and its SAS is confirmed, then the node
/// refuses its attach (`server_too_old`) before proving itself, and says
/// so in its status.
#[tokio::test(flavor = "multi_thread")]
async fn an_old_server_is_refused_after_its_sas() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let mut a = Fake::start().await;
    let n = start_node(&a, "srv-old", "", walled(Wall::Seatbelt)).await;
    let mut ca = a.next_conn().await;
    ready(&mut ca).await;

    // A protocol-2 server: hello → challenge(2) → whatever the node says.
    let old = std::sync::Arc::new(SigningKey::generate().unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<HandshakeFrame>();
    let k = old.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else { return };
            let Ok(mut ws) = tokio_tungstenite::accept_async(tcp).await else { continue };
            let Some(Ok(Message::Text(t))) = ws.next().await else { continue };
            let Ok(HandshakeFrame::Hello(h)) = serde_json::from_str::<HandshakeFrame>(&t) else { continue };
            let nonce_c = k2_node_proto::crypto::nonce_b64().unwrap();
            let sig = k.sign_domain(frames::DOMAIN_ATTACH_CONTROLLER, &[h.nonce_n.as_bytes(), h.node_fp.as_bytes(), nonce_c.as_bytes()]).unwrap();
            let ch = HandshakeFrame::Challenge(frames::Challenge { controller_fp: k.fingerprint(), nonce_c, sig, protocol: 2 });
            let _ = ws.send(Message::Text(serde_json::to_string(&ch).unwrap())).await;
            if let Some(Ok(Message::Text(t))) = ws.next().await {
                if let Ok(f) = serde_json::from_str::<HandshakeFrame>(&t) {
                    let _ = tx.send(f);
                }
            }
        }
    });
    let t = n.layout.tenant(1);
    t.ensure_state().unwrap();
    let pin = Pin {
        controller_fp: old.fingerprint(),
        controller_public_key_pem: old.spki_pem(),
        routes: vec![base],
        node_id: "node-old".into(),
        name: "mini-1".into(),
        enrolled_at: 1,
        sas: "123 456".into(),
        revoked: false,
        revoked_reason: None,
        labels: Default::default(),
        confirmed: true,
    };
    k2_node::identity::write_tenant_pin(&t.pin(), &pin, &TenantFlags::default()).unwrap();
    write_shared(&n, 1, &prefix(&old), "old.k2.dev", ShareState::Active, None);
    let said = tokio::time::timeout(Duration::from_secs(20), rx.recv()).await.unwrap().unwrap();
    match said {
        HandshakeFrame::Refused(r) => assert_eq!(r.code, codes::SERVER_TOO_OLD),
        other => panic!("the node proved itself to an old server: {other:?}"),
    }
    let line = eventually("refused in the status", || status(&n).servers.into_iter().find(|l| l.slot == 1 && l.state == "refused")).await;
    assert_eq!(line.link, "refused");
    let (_, flags) = k2_node::identity::read_tenant_pin(&t.pin()).unwrap().unwrap();
    assert_eq!(flags.refused.as_deref(), Some(codes::SERVER_TOO_OLD));
    assert_eq!(n.node.tenant(1).unwrap().job_gate(n.node.now(), true), Some(codes::SERVER_NOT_APPROVED));
}
