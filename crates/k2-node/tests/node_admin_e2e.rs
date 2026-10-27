//! Node admin over the loopback fake controller: the node as courier to
//! an in-process root executor (same uid here; peer rule = us), and the
//! verbs the node answers itself (S1/S3/S9). Temp dirs only.
// pending k2-node Windows port: k2-node (the compute node runtime) is Unix-only
// until its Windows port (Phase 3: Job Objects instead of uids and cgroups).
#![cfg(unix)]

mod common;

use std::sync::Arc;

use common::*;
use k2_node::admin::config::{self as acfg, AdminFile};
use k2_node::admin::exec::{ExecPaths, Executor, Platform, SystemPlatform};
use k2_node_proto::admin::{self, code, AdminResult, Class, Command};
use k2_node_proto::crypto::{self, SigningKey};
use k2_node_proto::frames::{AdminFrame, Control, Frame};
use serde_json::{json, Value};

fn cmd(key: &SigningKey, node_fp: &str, verb: &str, args: Value) -> admin::SignedCommand {
    let now = k2_node::util::now();
    let c = Command {
        v: admin::ADMIN_V,
        cmd_id: format!("e2e{}", &crypto::random_id().unwrap()[..8]),
        node_fp: node_fp.into(),
        controller_fp: key.fingerprint(),
        class: Class::Operate,
        verb: verb.into(),
        args,
        plan: None,
        proposed_by: "agent:it".into(),
        approved_by: None,
        approved_via: None,
        request_id: None,
        iat: now,
        exp: now + 300,
        nonce: admin::new_nonce().unwrap(),
    };
    admin::sign_controller(key, &c).unwrap()
}

async fn ask(c: &mut Conn, s: admin::SignedCommand) -> AdminResult {
    let body: Command = serde_json::from_slice(&crypto::unb64(&s.body_b64).unwrap()).unwrap();
    c.send(Frame::Admin(AdminFrame { cmd: s }));
    let fr = c.until(|f| matches!(f, Frame::AdminResult(r) if r.cmd_id == body.cmd_id || r.cmd_id.is_empty())).await;
    match fr.into_iter().last() {
        Some(Frame::AdminResult(r)) => r,
        o => panic!("{o:?}"),
    }
}

fn admin_toml(layout: &k2_node::paths::Layout, key: &SigningKey, level: &str) {
    acfg::write(
        &layout.config.join(acfg::ADMIN_FILE),
        &AdminFile {
            remote_admin: level.into(),
            controller_fp: Some(key.fingerprint()),
            controller_key: Some(crypto::b64(key.spki_der())),
            ..Default::default()
        },
    )
    .unwrap();
}

fn short_sock() -> std::path::PathBuf {
    std::path::PathBuf::from(format!("/tmp/k2na-e2e-{}.sock", &crypto::random_id().unwrap()[..10]))
}

#[tokio::test(flavor = "multi_thread")]
async fn offer_lists_admin_features_and_executor_unreachable_is_a_refusal() {
    let mut fake = Fake::start().await;
    let sock = short_sock();
    let s2 = sock.clone();
    let n = start_node(&fake, "adm-unreach", "", move |o| o.admin_socket = s2).await;
    let mut c = fake.next_conn().await;
    let fr = c.until(|f| matches!(f, Frame::Offer(_))).await;
    let Some(Frame::Offer(o)) = fr.last() else { panic!() };
    for f in ["admin", "self-update", "health"] {
        assert!(o.features.iter().any(|x| x == f), "{f} in {:?}", o.features);
    }
    admin_toml(&n.layout, &fake.key, "policy");
    let r = ask(&mut c, cmd(&fake.key, &n.node.key.fingerprint(), "control.pause", json!({}))).await;
    assert_eq!(r.code.as_deref(), Some(code::EXECUTOR_UNREACHABLE), "{r:?}");
    let _ = sock;
}

#[tokio::test(flavor = "multi_thread")]
async fn courier_relays_to_the_executor_which_checks_again() {
    let mut fake = Fake::start().await;
    let sock = short_sock();
    let s2 = sock.clone();
    let n = start_node(&fake, "adm-relay", "", move |o| o.admin_socket = s2).await;
    admin_toml(&n.layout, &fake.key, "policy");
    // The root executor, in-process (same uid as this test).
    let paths = ExecPaths { config_dir: n.layout.config.clone(), state_dir: n.dir.join("admin-state"), home: n.layout.home.clone(), binary: n.dir.join("bin/k2-node") };
    k2_node::admin::exec::ensure_state_dir(&paths.state_dir).unwrap();
    let exec = Arc::new(Executor::new(paths, Arc::new(SystemPlatform) as Arc<dyn Platform>));
    let l = k2_node::admin::server::bind(&sock, None).unwrap();
    let me = unsafe { libc::geteuid() };
    let e2 = exec.clone();
    std::thread::spawn(move || {
        let rule = k2_node::admin::server::PeerRule { allowed_uids: vec![me] };
        let _ = k2_node::admin::server::serve(&e2, &rule, l, &|| false);
    });
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    let fp = n.node.key.fingerprint();
    let r = ask(&mut c, cmd(&fake.key, &fp, "control.pause", json!({}))).await;
    assert_eq!(r.state, "done", "{r:?}");
    assert!(r.audit_seq.is_some(), "the executor audited it");
    // The node picks the owner's pause up from control.toml.
    let fr = c.until(|f| matches!(f, Frame::Offer(o) if o.control == Control::Paused)).await;
    assert!(!fr.is_empty());
    // Narrowing policy runs; widening is held for the owner (refused here).
    let r = ask(&mut c, cmd(&fake.key, &fp, "policy.set", json!({"max_parallel": 1}))).await;
    assert_eq!(r.state, "done", "{r:?}");
    let r = ask(&mut c, cmd(&fake.key, &fp, "policy.set", json!({"max_parallel": 9}))).await;
    assert_eq!(r.code.as_deref(), Some(code::APPROVAL_REQUIRED), "{r:?}");
    // A command signed by another key: the node refuses before the executor.
    let other = SigningKey::generate().unwrap();
    let r = ask(&mut c, cmd(&other, &fp, "control.resume", json!({}))).await;
    assert_eq!(r.code.as_deref(), Some(code::BAD_CONTROLLER_SIGNATURE));
    // Persisted narrowing reaches the offer.
    let r = ask(&mut c, cmd(&fake.key, &fp, "policy.narrow", json!({"mem_bytes": 1u64 << 30}))).await;
    assert_eq!(r.state, "done", "{r:?}");
    let fr = c.until(|f| matches!(f, Frame::Offer(o) if o.caps.mem_bytes == 1 << 30)).await;
    assert!(!fr.is_empty());
    let lines = exec.audit().verify().unwrap();
    assert!(lines.len() >= 6, "{}", lines.len());
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test(flavor = "multi_thread")]
async fn node_answered_verbs_ps_kill_gc_show_and_levels() {
    let mut fake = Fake::start().await;
    let n = start_node(&fake, "adm-node", "", |o| o.admin_socket = short_sock()).await;
    let mut c = fake.next_conn().await;
    ready(&mut c).await;
    let fp = n.node.key.fingerprint();
    // No admin.toml: level off. Reads work, operate verbs don't.
    let r = ask(&mut c, cmd(&fake.key, &fp, "policy.show", json!({}))).await;
    assert_eq!(r.state, "done", "{r:?}");
    assert_eq!(r.result["policy"]["max_parallel"], 2);
    let r = ask(&mut c, cmd(&fake.key, &fp, "gc", json!({}))).await;
    assert_eq!(r.code.as_deref(), Some(code::REMOTE_ADMIN_OFF));
    admin_toml(&n.layout, &fake.key, "policy");
    // A running job shows in ps and can be killed.
    c.send(Frame::Assign(assign(plan("k1", &["/bin/sh", "-c", "echo up; sleep 1000"]), 1)));
    c.until(|f| matches!(f, Frame::Log(l) if l.job_id == "k1")).await;
    let r = ask(&mut c, cmd(&fake.key, &fp, "ps", json!({}))).await;
    assert_eq!(r.state, "done", "{r:?}");
    assert_eq!(r.result["jobs"][0]["job_id"], "k1");
    assert!(r.result["stuck"].as_array().unwrap().is_empty());
    let r = ask(&mut c, cmd(&fake.key, &fp, "jobs.kill", json!({"job_id": "nope"}))).await;
    assert_eq!(r.code.as_deref(), Some(code::BAD_ARGS));
    let r = ask(&mut c, cmd(&fake.key, &fp, "jobs.kill", json!({"job_id": "k1"}))).await;
    assert_eq!(r.state, "done", "{r:?}");
    let fr = run_to_receipt(&mut c, "k1").await;
    assert_eq!(states_of(&fr, "k1").last().unwrap().state, k2_node_proto::frames::JobState::Cancelled);
    // gc: nothing is old enough; args are strict.
    let r = ask(&mut c, cmd(&fake.key, &fp, "gc", json!({"older_days": 1}))).await;
    assert_eq!(r.state, "done", "{r:?}");
    assert_eq!(r.result["job_dirs"], 0);
    assert!(n.layout.job_dir("k1", 1).exists(), "a fresh job tree is kept");
    let r = ask(&mut c, cmd(&fake.key, &fp, "gc", json!({"older_days": 0}))).await;
    assert_eq!(r.code.as_deref(), Some(code::BAD_ARGS));
    // Replay of a node-answered command.
    let s = cmd(&fake.key, &fp, "ps", json!({}));
    assert_eq!(ask(&mut c, s.clone()).await.state, "done");
    assert_eq!(ask(&mut c, s).await.code.as_deref(), Some(code::REPLAYED));
    // Wrong node.
    let r = ask(&mut c, cmd(&fake.key, &"0".repeat(64), "ps", json!({}))).await;
    assert_eq!(r.code.as_deref(), Some(code::WRONG_NODE));
}
