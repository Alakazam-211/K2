//! App-governed Spaces S1/S2/S6 (0.46.7) end to end against the fake Sew
//! mgmt socket (Sew 0.6.0's I7 and I8 shapes, flag `app_governed_spaces`):
//! - S1: an App's chat key reaches Sew as `max_level` + `uploads` on
//!   `POST /mgmt/v1/apps/{app_id}/settings`, only once Sew reports the flag;
//! - S2: a keyed App's person (no chat grant of their own) is provisioned
//!   first (`principals/provision`, the S27 outbox) and the pass gate says
//!   "pending" until Sew confirmed the member, then "ready";
//! - S6: revoking the key (and removing the App) sends
//!   `POST /mgmt/v1/sessions/revoke {app_id}`; an older Sew never sees the
//!   field.
//!
//! Tests drive `tool_hook_worker::{pass_once, reconcile_once}` directly.
//! Isolation: `TempHome::short()`; never the real `~/.k2` or `/run/sew`.

#![cfg(unix)]

#[path = "support/fake_sew.rs"]
mod fake_sew;

use std::time::Duration;

use fake_sew::FakeSew;
use k2_core::skin::{self, app_chat_key as ck};
use k2_core::test_env::TempHome;
use k2_core::tool_hook::{self as hook, KeyedChatGate, CHAT_KIND};
use k2_daemon::tool_hook_worker::{self as worker, WorkerConfig};
use serde_json::json;

fn cfg() -> WorkerConfig {
    WorkerConfig {
        tick: Duration::from_millis(50),
        backoff_base: Duration::from_millis(20),
        backoff_cap: Duration::from_millis(200),
        reconcile_every: Duration::from_secs(3600),
        reconcile_debounce: Duration::from_millis(10),
        dep_wait: Duration::from_secs(3600),
    }
}

struct Env {
    _home: TempHome,
    sew: FakeSew,
}

async fn setup() -> Env {
    let home = TempHome::short();
    hook::allow_target_off_linux_for_tests(true);
    let sew = FakeSew::start(&home.path().join("s")).await;
    hook::set_target(CHAT_KIND, true, Some(sew.socket.to_str().unwrap()), Some(sew.token_file.to_str().unwrap()), "owner-token")
        .expect("enable chat target");
    Env { _home: home, sew }
}

fn rows() -> Vec<hook::outbox::OutboxRow> {
    let mut r = hook::log(500, None).expect("log");
    r.reverse();
    r
}

async fn drain(env: &Env) {
    for _ in 0..40 {
        hook::force_due_for_tests().unwrap();
        worker::pass_once(&cfg()).await.expect("pass");
        if rows().iter().all(|r| r.state != "pending") {
            return;
        }
    }
    panic!("outbox never drained: {:?}\nfake log:\n{}", rows(), env.sew.log());
}

fn want(level: ck::Level, uploads: bool, max: u32) -> ck::Want {
    ck::Want { caps: vec!["chat:read".into(), "voice:join".into()], max_level: level, uploads, max_members: max }
}

fn app_revokes(env: &Env) -> Vec<String> {
    env.sew.revoked().into_iter().filter_map(|r| r.app_id).collect()
}

/// S1 + S6 on a Sew 0.6.0: the key's ceiling reaches Sew's App settings
/// (I7), a change goes again, and a revoke ends the App's sessions by
/// `app_id` (I8). Nothing K2 sends is refused by the fake.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_key_reaches_sew_and_its_revoke_ends_the_apps_sessions() {
    let env = setup().await;
    env.sew.set_features(&["app_governed_spaces", "app_origin", "home_client"]);
    let svc = uuid::Uuid::new_v4().to_string();
    worker::reconcile_once().await.expect("sweep: K2 learns the flags");
    ck::set(&svc, &want(ck::Level::Moderator, true, 50), "owner").expect("key on");
    worker::reconcile_once().await.expect("sweep");
    drain(&env).await;
    let got = env.sew.app_settings(&svc).expect("I7 sent");
    assert_eq!((got["max_level"].clone(), got["uploads"].clone()), (json!("moderator"), json!(true)), "{got}");

    // A narrowing goes again, once.
    ck::set(&svc, &want(ck::Level::Member, false, 50), "owner").expect("narrow");
    worker::reconcile_once().await.expect("sweep");
    drain(&env).await;
    let got = env.sew.app_settings(&svc).expect("I7 sent");
    assert_eq!((got["max_level"].clone(), got["uploads"].clone()), (json!("member"), json!(false)), "{got}");
    let before = env.sew.posts().len();
    worker::reconcile_once().await.expect("idle sweep");
    drain(&env).await;
    assert_eq!(env.sew.posts().iter().skip(before).filter(|c| c.path.contains("/apps/")).count(), 0, "nothing owed");

    // Revoke: Sew ends every session of this App, and only by app_id.
    assert!(ck::revoke(&svc, "owner").unwrap().sessions_revoke_queued);
    drain(&env).await;
    assert_eq!(app_revokes(&env), vec![svc.clone()], "{}", env.sew.log());
    let body = env.sew.posts().into_iter().find(|c| c.path == "/mgmt/v1/sessions/revoke").expect("the revoke call").body;
    assert_eq!(body, json!({ "app_id": svc }), "exactly I8's body");
    env.sew.assert_clean();
}

/// Never by version: on a Sew without `app_governed_spaces`, the key's
/// ceiling never goes and a revoke sends no `app_id` (the fake would refuse
/// it like an older Sew and record a bad body).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_older_sew_never_sees_the_ceiling_or_an_app_id_revoke() {
    let env = setup().await;
    env.sew.set_features(&["app_origin", "home_client"]);
    let svc = uuid::Uuid::new_v4().to_string();
    worker::reconcile_once().await.expect("sweep");
    ck::set(&svc, &want(ck::Level::Admin, true, 50), "owner").expect("key row (the route refuses this without the flag)");
    worker::reconcile_once().await.expect("sweep");
    drain(&env).await;
    assert!(env.sew.app_settings(&svc).is_none_or(|s| s.get("max_level").is_none() && s.get("uploads").is_none()), "{}", env.sew.log());
    assert!(!ck::revoke(&svc, "owner").unwrap().sessions_revoke_queued);
    drain(&env).await;
    assert!(app_revokes(&env).is_empty());
    env.sew.assert_clean();
}

/// S2: a keyed App's person needs no chat grant: the gate writes one, the
/// S27 outbox provisions `k2:principal:<id>` first, the gate answers
/// pending until Sew confirmed it, then ready. The key's member ceiling
/// holds. Removing the App ends its sessions by `app_id`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_keyed_apps_person_is_provisioned_before_their_first_pass() {
    let env = setup().await;
    env.sew.set_features(&["app_governed_spaces"]);
    worker::reconcile_once().await.expect("sweep");
    let svc = uuid::Uuid::new_v4().to_string();
    let kim = skin::add_principal_for_app("kim", None, &svc, None, "test").unwrap();
    let lee = skin::add_principal_for_app("lee", None, &svc, None, "test").unwrap();
    let ext = format!("k2:principal:{}", kim.id);
    assert_eq!(hook::keyed_chat_gate(&kim.id, &svc, "kim").unwrap(), KeyedChatGate::NoGrant, "no key: today's rule");
    ck::set(&svc, &want(ck::Level::Moderator, true, 1), "owner").expect("key on");

    assert_eq!(hook::keyed_chat_gate(&kim.id, &svc, "kim").unwrap(), KeyedChatGate::Pending);
    assert!(env.sew.member(&ext).is_none(), "a pass never creates a member: K2 provisions first");
    drain(&env).await;
    let m = env.sew.member(&ext).expect("provisioned by K2");
    assert_eq!((m.username.as_str(), m.deactivated), ("kim", false));
    assert_eq!(hook::keyed_chat_gate(&kim.id, &svc, "kim").unwrap(), KeyedChatGate::Ready);
    // The ceiling: one person through this key.
    assert_eq!(hook::keyed_chat_gate(&lee.id, &svc, "lee").unwrap(), KeyedChatGate::Limit(1));
    assert!(env.sew.member(&format!("k2:principal:{}", lee.id)).is_none());

    // The App is removed: its key goes off, kim (only this App) loses chat,
    // and Sew ends the App's sessions by app_id.
    skin::app_chat::app_removed(&svc, "system:app-removed").unwrap();
    drain(&env).await;
    assert!(env.sew.member(&ext).expect("kept").deactivated, "deactivated, never deleted");
    assert_eq!(app_revokes(&env), vec![svc.clone()], "{}", env.sew.log());
    assert!(!ck::get(&svc).unwrap().expect("row kept").enabled);
    env.sew.assert_clean();
}
