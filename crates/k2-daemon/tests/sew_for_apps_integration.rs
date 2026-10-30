//! Sew for Apps (0.46.6), end to end against the fake Sew mgmt socket:
//! - A3: K2 is the only source of the name and picture Sew shows (the
//!   display-name rule, the avatar ops, triggers on profile writes, the
//!   reconcile backstop after a reinstall).
//!
//! Tests drive `tool_hook_worker::{pass_once, reconcile_once}` directly.
//! Isolation: `TempHome::short()`; never the real `~/.k2` or `/run/sew`.

#![cfg(unix)]

#[path = "support/fake_sew.rs"]
mod fake_sew;

use std::time::Duration;

use fake_sew::FakeSew;
use k2_core::skin::{self, SkinGrantWrite};
use k2_core::test_env::TempHome;
use k2_core::tool_hook::{self as hook, CHAT_KIND};
use k2_daemon::tool_hook_worker::{self as worker, WorkerConfig};
use sha2::{Digest, Sha256};

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
    hook::set_target(
        CHAT_KIND,
        true,
        Some(sew.socket.to_str().unwrap()),
        Some(sew.token_file.to_str().unwrap()),
        "owner-token",
    )
    .expect("enable chat target");
    Env { _home: home, sew }
}

fn chat(principal_id: &str) -> skin::SkinGrant {
    skin::create_grant(&SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: principal_id.into(),
        kind: "chat".into(),
        target_id: String::new(),
        role: None,
        scope: Some("member".into()),
        created_by: Some("owner-token".into()),
    })
    .expect("chat grant")
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

fn pext(id: &str) -> String {
    format!("k2:principal:{id}")
}

fn sha(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// A3 (P1, P2): the profile display name wins over the full name, and the
/// picture arrives with the first provision.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provision_carries_the_profile_name_and_picture() {
    let env = setup().await;
    let p = skin::add_principal("pia").unwrap();
    skin::set_principal_full_name("pia", Some("Pia Long")).unwrap();
    skin::profile::set_display_name(&p.id, Some("Pia L")).unwrap();
    let jpeg = b"\xff\xd8fake-jpeg-pia\xff\xd9".to_vec();
    let hash = skin::profile::set_image(&p.id, &jpeg).unwrap();
    // No chat yet: nothing queued for her.
    assert!(rows().iter().all(|r| r.subject_key != pext(&p.id)), "{:?}", rows());
    chat(&p.id);
    drain(&env).await;
    let m = env.sew.member(&pext(&p.id)).expect("provisioned");
    assert_eq!(m.display_name, "Pia L", "the profile display name, not the full name");
    assert_eq!(env.sew.avatar(&pext(&p.id)), Some((sha(&jpeg), jpeg.len())), "{}", env.sew.log());
    assert_eq!(hash, sha(&jpeg));
    env.sew.assert_clean();

    // Clearing the display name falls back to the full name.
    skin::profile::set_display_name(&p.id, None).unwrap();
    drain(&env).await;
    assert_eq!(env.sew.member(&pext(&p.id)).unwrap().display_name, "Pia Long");
    env.sew.assert_clean();
}

/// A3: profile writes reach Sew (name update, picture set, picture clear);
/// a picture replaced before delivery sends only the newest.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_writes_reach_sew_and_a_replaced_picture_is_superseded() {
    let env = setup().await;
    let p = skin::add_principal("quin").unwrap();
    chat(&p.id);
    drain(&env).await;
    assert_eq!(env.sew.member(&pext(&p.id)).unwrap().display_name, "quin", "username when nothing else");
    assert_eq!(env.sew.avatar(&pext(&p.id)), None, "no picture, no push");

    skin::profile::set_display_name(&p.id, Some("Q")).unwrap();
    let a = b"jpeg-A".to_vec();
    let b = b"jpeg-B-newer".to_vec();
    skin::profile::set_image(&p.id, &a).unwrap();
    skin::profile::set_image(&p.id, &b).unwrap();
    drain(&env).await;
    assert_eq!(env.sew.member(&pext(&p.id)).unwrap().display_name, "Q");
    assert_eq!(env.sew.avatar(&pext(&p.id)), Some((sha(&b), b.len())), "only the newest picture went");
    let sets: Vec<_> = rows().into_iter().filter(|r| r.op == "avatar_set" && r.subject_key == pext(&p.id)).collect();
    assert_eq!(sets.len(), 2, "{sets:?}");
    assert_eq!(sets[0].state, "superseded", "the older picture finished unsent: {sets:?}");
    assert_eq!(sets[1].state, "done");
    let pushes = env.sew.posts().into_iter().filter(|c| c.path == "/mgmt/v1/principals/avatar").count();
    assert_eq!(pushes, 1, "{}", env.sew.log());

    assert!(skin::profile::clear_image(&p.id).unwrap());
    drain(&env).await;
    assert_eq!(env.sew.avatar(&pext(&p.id)), None, "cleared in Sew too");
    assert!(!skin::profile::clear_image(&p.id).unwrap(), "twice: nothing to clear");
    env.sew.assert_clean();
}

/// A3 reconcile: a new Sew install (purge, reinstall) gets every member and
/// picture again from K2.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reinstall_gets_the_pictures_again_through_reconcile() {
    let env = setup().await;
    let p = skin::add_principal("rae").unwrap();
    let jpeg = b"jpeg-rae".to_vec();
    skin::profile::set_image(&p.id, &jpeg).unwrap();
    chat(&p.id);
    worker::reconcile_once().await.expect("first sweep");
    drain(&env).await;
    assert!(env.sew.avatar(&pext(&p.id)).is_some());
    // A sweep with nothing changed plans no picture.
    worker::reconcile_once().await.expect("idle sweep");
    assert!(rows().iter().filter(|r| r.op == "avatar_set").count() == 1, "{:?}", rows());

    env.sew.reinstall("987654321");
    worker::reconcile_once().await.expect("sweep after reinstall");
    drain(&env).await;
    assert!(env.sew.member(&pext(&p.id)).is_some(), "re-provisioned");
    assert_eq!(env.sew.avatar(&pext(&p.id)), Some((sha(&jpeg), jpeg.len())), "{}", env.sew.log());
    env.sew.assert_clean();
}

// ── A6: the identity map ─────────────────────────────────────────────

fn mapped(ext: &str) -> Option<hook::members::MemberRow> {
    hook::chat_member(ext).unwrap()
}

/// A6: the worker maps every delivered provision to Sew's member id; a
/// deleted person is a tombstone; the same person invited again is a new
/// principal, so a new member; reconcile keeps the map.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_identity_map_follows_provisions_tombstones_and_reinvites() {
    let env = setup().await;
    let p = skin::add_principal("sam").unwrap();
    chat(&p.id);
    drain(&env).await;
    let m = env.sew.member(&pext(&p.id)).expect("member");
    let row = mapped(&pext(&p.id)).expect("mapped by the worker");
    assert_eq!((row.member_id.clone(), row.state.as_str(), row.username.as_str()), (m.member_id.to_string(), "active", "sam"));
    // A sweep (Sew's list is canonical) keeps the same mapping.
    worker::reconcile_once().await.expect("sweep");
    drain(&env).await;
    assert_eq!(mapped(&pext(&p.id)).unwrap().member_id, m.member_id.to_string());

    // Chat taken away: deactivated, same id; given again: the same member.
    let g = skin::list_grants().unwrap().into_iter().find(|g| g.kind == "chat" && g.subject_id == p.id).unwrap();
    skin::set_grant_enabled_as(&g.id, false, "owner-token").unwrap();
    drain(&env).await;
    assert_eq!(mapped(&pext(&p.id)).unwrap().state, "deactivated");
    skin::set_grant_enabled_as(&g.id, true, "owner-token").unwrap();
    drain(&env).await;
    let back = mapped(&pext(&p.id)).unwrap();
    assert_eq!((back.state.as_str(), back.member_id.clone()), ("active", m.member_id.to_string()), "reactivated, same member id");

    // Deleted: a tombstone (the member is deactivated in Sew, history stays).
    assert!(skin::remove_principal("sam").unwrap());
    drain(&env).await;
    assert!(env.sew.member(&pext(&p.id)).unwrap().deactivated);
    assert_eq!(mapped(&pext(&p.id)).unwrap().state, "deleted");
    worker::reconcile_once().await.expect("sweep");
    assert_eq!(mapped(&pext(&p.id)).unwrap().state, "deleted", "a sweep keeps the tombstone");

    // Invited again under the same name: a new principal, a new member.
    let p2 = skin::add_principal("sam").unwrap();
    assert_ne!(p2.id, p.id, "K2 never reuses a principal id");
    chat(&p2.id);
    drain(&env).await;
    let m2 = env.sew.member(&pext(&p2.id)).expect("a new member");
    assert_ne!(m2.member_id, m.member_id);
    assert_eq!(mapped(&pext(&p2.id)).unwrap().state, "active");
    assert_eq!(mapped(&pext(&p.id)).unwrap().state, "deleted", "the old history stays with the tombstone");
    env.sew.assert_clean();
}

/// A6 (Q12): removing an App deactivates the chat of people whose only App
/// it was; people with another login keep chat.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_an_app_deactivates_its_sole_users_chat() {
    let env = setup().await;
    let svc = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    let only = skin::add_principal_for_app("tia", None, &svc, None, "test").unwrap();
    let both = skin::add_principal_for_app("uma", None, &svc, None, "test").unwrap();
    skin::create_grant(&SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: both.id.clone(),
        kind: "app".into(),
        target_id: other.clone(),
        role: None,
        scope: Some("[]".into()),
        created_by: None,
    })
    .unwrap();
    chat(&only.id);
    chat(&both.id);
    drain(&env).await;
    assert_eq!(skin::app_chat::app_removed(&svc, "system:app-removed").unwrap(), 1);
    drain(&env).await;
    assert!(env.sew.member(&pext(&only.id)).unwrap().deactivated, "deactivated, never deleted");
    assert!(!env.sew.member(&pext(&both.id)).unwrap().deactivated, "another App keeps her chat");
    assert_eq!(mapped(&pext(&only.id)).unwrap().state, "deactivated");
    assert_eq!(skin::app_chat::app_removed(&svc, "system:app-removed").unwrap(), 0, "twice: nothing left");
    env.sew.assert_clean();
}

// ── Decision 13: home client and per-App settings ────────────────────

/// D13: each setting reaches Sew only once Sew reports the feature flag
/// for it (never by version), once per change, and again after a
/// reinstall. The origin waits for `app_origin`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn home_settings_follow_sews_feature_flags_never_its_version() {
    let env = setup().await;
    let hearth = uuid::Uuid::new_v4().to_string();
    let jobs = uuid::Uuid::new_v4().to_string();
    assert!(hook::set_home_client(Some(&hearth), "owner-token").unwrap());
    assert!(hook::set_app_home(&jobs, true, "Jobs", "owner-token").unwrap());
    assert!(!hook::set_app_home(&jobs, true, "Jobs", "owner-token").unwrap(), "the same value: no change");
    assert_eq!(hook::note_app_origins(&[(jobs.clone(), Some("wss://jobs.example.com".into()), "Jobs".into())]).unwrap(), 1);

    // No flags (an older Sew): nothing is sent.
    worker::reconcile_once().await.expect("sweep without flags");
    drain(&env).await;
    assert_eq!(env.sew.home_client(), None, "an older Sew never sees the call");
    assert!(rows().iter().all(|r| r.op != "home_client" && r.op != "app_settings"), "{:?}", rows());

    // home_client only: the home client and the App-only mark, no origin.
    env.sew.set_features(&["home_client"]);
    worker::reconcile_once().await.expect("sweep with home_client");
    drain(&env).await;
    assert_eq!(env.sew.home_client(), Some(Some(hearth.clone())));
    // AG-21: the home client goes to Sew under the person who set it.
    let hc: Vec<_> = env.sew.posts().into_iter().filter(|c| c.path == "/mgmt/v1/home-client").collect();
    assert_eq!(hc.len(), 1, "{hc:?}");
    assert_eq!(hc[0].actor.as_deref(), Some("owner-token"), "Sew-Actor = who set the home client");
    assert_eq!(env.sew.app_settings(&jobs), Some(serde_json::json!({"hidden_from_home": true, "display_name": "Jobs"})));
    assert!(hook::sew_has("home_client").unwrap() && !hook::sew_has("app_origin").unwrap());

    // app_origin appears: the origin goes, once.
    env.sew.set_features(&["app_origin", "home_client"]);
    worker::reconcile_once().await.expect("sweep with app_origin");
    drain(&env).await;
    assert_eq!(env.sew.app_settings(&jobs).unwrap()["origin"], serde_json::json!("wss://jobs.example.com"));
    assert!(hook::home_settings().unwrap().iter().all(|s| s.delivered));
    let before = env.sew.posts().len();
    worker::reconcile_once().await.expect("idle sweep");
    drain(&env).await;
    assert_eq!(env.sew.posts().iter().skip(before).filter(|c| c.path.contains("home") || c.path.contains("/apps/")).count(), 0, "nothing owed");

    // A change goes once; clearing the home client sends null.
    hook::set_app_home(&jobs, false, "Jobs", "owner-token").unwrap();
    hook::set_home_client(None, "owner-token").unwrap();
    worker::reconcile_once().await.expect("sweep");
    drain(&env).await;
    assert_eq!(env.sew.app_settings(&jobs).unwrap()["hidden_from_home"], serde_json::json!(false));
    assert_eq!(env.sew.home_client(), Some(None));

    // A new install owes every setting again.
    env.sew.reinstall("555555555");
    env.sew.set_features(&["app_origin", "home_client"]);
    worker::reconcile_once().await.expect("sweep after reinstall");
    drain(&env).await;
    assert_eq!(env.sew.app_settings(&jobs).unwrap()["origin"], serde_json::json!("wss://jobs.example.com"));
    env.sew.assert_clean();
}

/// App-governed Spaces S7 (AG-13c): an App that loses its public URL sends
/// `origin: null`, so Sew drops the old one; a new URL goes again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_public_url_clears_the_origin_in_sew() {
    let env = setup().await;
    env.sew.set_features(&["app_origin", "home_client"]);
    let jobs = uuid::Uuid::new_v4().to_string();
    hook::note_app_origins(&[(jobs.clone(), Some("wss://jobs.example.com".into()), "Jobs".into())]).unwrap();
    worker::reconcile_once().await.expect("sweep");
    drain(&env).await;
    assert_eq!(env.sew.app_settings(&jobs).unwrap()["origin"], serde_json::json!("wss://jobs.example.com"));

    assert_eq!(hook::note_app_origins(&[(jobs.clone(), None, "Jobs".into())]).unwrap(), 1);
    worker::reconcile_once().await.expect("sweep");
    drain(&env).await;
    assert_eq!(env.sew.app_settings(&jobs).unwrap()["origin"], serde_json::Value::Null, "cleared: {}", env.sew.log());
    assert!(hook::home_settings().unwrap().iter().all(|s| s.delivered));

    hook::note_app_origins(&[(jobs.clone(), Some("wss://jobs.box.k2.dev".into()), "Jobs".into())]).unwrap();
    worker::reconcile_once().await.expect("sweep");
    drain(&env).await;
    assert_eq!(env.sew.app_settings(&jobs).unwrap()["origin"], serde_json::json!("wss://jobs.box.k2.dev"));
    env.sew.assert_clean();
}

/// Rule 3 (Sew 2026-10-28): until Sew reports `app_scoped_sessions`, an App
/// may not give its users chat (it would see all chat they can); the owner
/// still can.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn app_granted_chat_waits_for_app_scoped_sessions() {
    use skin::app_chat::{self as ac, Caller, Refusal, Scope};
    let env = setup().await;
    let svc = uuid::Uuid::new_v4().to_string();
    skin::add_principal_for_app("vic", None, &svc, None, "test").unwrap();
    skin::add_principal_for_app("wen", None, &svc, None, "test").unwrap();
    ac::grant(&svc, None, "owner-token").unwrap();
    worker::reconcile_once().await.expect("sweep without the flag");
    let r = ac::set(&Caller::App, Some(&svc), "vic", Scope::Member, "app:jobs", "app");
    assert_eq!(r, Err(Refusal::NotAppScoped), "refused before the flag");
    // The owner may, and is told what the App can see.
    let o = ac::set(&Caller::Owner, Some(&svc), "wen", Scope::Member, "owner-token", "cli").unwrap();
    assert!(!o.app_scoped);
    assert!(o.warning.as_deref().is_some_and(|w| w.contains("this App can see all chat this person can")), "{o:?}");
    env.sew.set_features(&["app_scoped_sessions"]);
    worker::reconcile_once().await.expect("sweep with the flag");
    let ok = ac::set(&Caller::App, Some(&svc), "vic", Scope::Member, "app:jobs", "app").unwrap();
    assert!(ok.changed && ok.app_scoped && ok.warning.is_none(), "{ok:?}");
    drain(&env).await;
    assert!(env.sew.member(&pext(&ok.principal_id)).is_some());
    env.sew.assert_clean();
}

// ── A9: chat.createChannel (Sew 0.5.0, flag app_channel_create) ─────

const SPACE: &str = "48831765147549697";

/// The App (a fresh service id) and its remote pass. The process-wide test
/// DB, not a scoped one: the handler's blocking steps (the audit row) run
/// on other threads.
struct ChannelFx {
    svc: String,
    pass: String,
}

fn channel_fx() -> ChannelFx {
    let svc = uuid::Uuid::new_v4().to_string();
    let room = uuid::Uuid::new_v4().to_string();
    let pass = skin::link::mint_remote_pass(&svc, "jobs", &["chat:channels".to_string()], &[room], 3600).unwrap().raw;
    ChannelFx { svc, pass }
}

async fn create_channel(raw: &str, body: serde_json::Value) -> (u16, serde_json::Value) {
    let pass = k2_daemon::apps::gateway::resolve_pass(raw);
    let r = k2_daemon::chat_channel_routes::handle(pass, None, true, body.to_string().into_bytes()).await;
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", r.body)))
}

fn err_code(v: &serde_json::Value) -> String {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}")).to_string()
}

fn channel_posts(env: &Env) -> Vec<String> {
    env.sew
        .posts()
        .into_iter()
        .filter(|c| c.path == "/mgmt/v1/channels/create" || (c.path == "/mgmt/v1/spaces/members/add" && c.actor.as_deref().is_some_and(|a| a.starts_with("app"))))
        .map(|c| c.path)
        .collect()
}

/// A9: refused until Sew reports `app_channel_create`; then the listed
/// users not yet in the App's Space are added to it FIRST, the channel is
/// made with the users now in it, and Sew's `not_added` and `skipped`
/// come back as usernames. A repeat answers `created: false` with the
/// channel's current name and visibility.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_app_makes_a_channel_in_its_own_space() {
    use serde_json::json;
    let env = setup().await;
    let f = channel_fx();
    let mut ids = std::collections::HashMap::new();
    for u in ["dana", "eli", "fay", "gus"] {
        ids.insert(u, skin::add_principal_for_app(u, None, &f.svc, None, "test").unwrap().id);
    }
    // dana, eli and gus have chat (provisioned); fay doesn't.
    for u in ["dana", "eli", "gus"] {
        chat(&ids[u]);
    }
    drain(&env).await;
    env.sew.add_app_space(SPACE, &f.svc);
    env.sew.ban_in_space(SPACE, &pext(&ids["gus"]));
    hook::app_space_bind(&f.svc, SPACE, "owner-token").unwrap();
    // hal gets chat now: his provision is still queued when the App calls.
    ids.insert("hal", skin::add_principal_for_app("hal", None, &f.svc, None, "test").unwrap().id);
    chat(&ids["hal"]);
    let ask = json!({"slug": "job-1042", "name": "Job 1042", "private": true, "members": ["dana", "eli", "fay", "gus", "hal", "nobody"]});

    // An older Sew (no flag): refused, and Sew is never asked.
    worker::reconcile_once().await.expect("sweep without the flag");
    let (st, v) = create_channel(&f.pass, ask.clone()).await;
    assert_eq!((st, err_code(&v).as_str()), (409, "sew_feature_missing"), "{v}");
    assert!(channel_posts(&env).is_empty(), "{}", env.sew.log());

    env.sew.set_features(&["app_channel_create"]);
    worker::reconcile_once().await.expect("sweep with the flag");
    let (st, v) = create_channel(&f.pass, ask.clone()).await;
    assert_eq!(st, 200, "{v}\n{}", env.sew.log());
    assert_eq!((v["ok"].clone(), v["app"].clone(), v["spaceId"].clone()), (json!(true), json!(f.svc), json!(SPACE)));
    assert_eq!(
        (v["slug"].clone(), v["name"].clone(), v["private"].clone(), v["created"].clone()),
        (json!("job-1042"), json!("Job 1042"), json!(true), json!(true))
    );
    assert!(v["channelId"].as_str().is_some_and(|c| !c.is_empty()), "{v}");
    assert_eq!(v["added"], json!(["dana", "eli"]), "{v}");
    assert_eq!(v["skipped"], json!(["gus"]), "banned in the Space: {v}");
    assert_eq!(
        v["notAdded"],
        json!([{"username": "fay", "reason": "no_chat"}, {"username": "nobody", "reason": "not_found"}, {"username": "hal", "reason": "not_provisioned"}]),
        "{v}"
    );
    // The Space add came first, then the create with the users in the Space.
    assert_eq!(channel_posts(&env), ["/mgmt/v1/spaces/members/add", "/mgmt/v1/channels/create"], "{}", env.sew.log());
    let calls = env.sew.posts();
    let add = calls.iter().find(|c| c.path == "/mgmt/v1/spaces/members/add" && c.actor.as_deref().is_some_and(|a| a.starts_with("app"))).unwrap();
    assert_eq!(add.body["app_id"], json!(f.svc));
    assert_eq!(add.body["external_ids"], json!([pext(&ids["dana"]), pext(&ids["eli"]), pext(&ids["gus"]), pext(&ids["hal"])]));
    assert_eq!(add.actor.as_deref(), Some(format!("app-remote:{}", f.svc).as_str()), "Sew-Actor = the App");
    let create = calls.iter().find(|c| c.path == "/mgmt/v1/channels/create").unwrap();
    assert_eq!(create.body["members"], json!([pext(&ids["dana"]), pext(&ids["eli"])]), "only the users in the Space");
    let ch = env.sew.channel(SPACE, "job-1042").expect("made");
    assert!(ch.private);
    assert_eq!(ch.members.len(), 2);
    assert_eq!(skin::app_channels::audit_rows(&f.svc, 5).unwrap().len(), 1, "audited");

    // A repeat (another name and visibility): created false, the current
    // name and visibility; nobody is sent to the Space again; a member a
    // moderator removed by hand comes back not_space_member.
    env.sew.remove_from_space(SPACE, &pext(&ids["eli"]));
    let before = channel_posts(&env).len();
    let (st, v) = create_channel(&f.pass, json!({"slug": "job-1042", "name": "Renamed", "private": false, "members": ["dana", "eli", "gus"]})).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!((v["created"].clone(), v["name"].clone(), v["private"].clone()), (json!(false), json!("Job 1042"), json!(true)), "{v}");
    assert_eq!(v["added"], json!(["dana"]), "already in: {v}");
    assert_eq!(v["skipped"], json!(["gus"]), "never sent again: {v}");
    assert_eq!(v["notAdded"], json!([{"username": "eli", "reason": "not_space_member"}]), "{v}");
    assert_eq!(channel_posts(&env)[before..], ["/mgmt/v1/channels/create"], "no Space add: everyone listed is in, out or banned");
    assert_eq!(env.sew.channel(SPACE, "job-1042").unwrap().name, "Job 1042");
    env.sew.assert_clean();
}

/// A9: Sew's 409s map to K2 codes; another App's Space (K2's binding) and
/// a guest's pass never reach Sew.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channel_create_refusals() {
    use serde_json::json;
    let env = setup().await;
    let f = channel_fx();
    env.sew.set_features(&["app_channel_create"]);
    worker::reconcile_once().await.expect("sweep");
    let (unbound, theirs, mine_k2_theirs_sew) = ("48831765147549701", "48831765147549702", "48831765147549703");
    let other = uuid::Uuid::new_v4().to_string();
    // K2 binds two to this App; Sew has one unbound and one on another App.
    env.sew.add_space(unbound);
    env.sew.add_app_space(mine_k2_theirs_sew, "other-app");
    env.sew.add_app_space(theirs, &other);
    hook::app_space_bind(&f.svc, unbound, "owner-token").unwrap();
    hook::app_space_bind(&f.svc, mine_k2_theirs_sew, "owner-token").unwrap();
    hook::app_space_bind(&other, theirs, "owner-token").unwrap();

    let ask = |space: &str| json!({"space": space, "slug": "crew", "name": "Crew"});
    let (st, v) = create_channel(&f.pass, ask(unbound)).await;
    assert_eq!((st, err_code(&v).as_str()), (409, "space_unbound"), "{v}");
    let (st, v) = create_channel(&f.pass, ask(mine_k2_theirs_sew)).await;
    assert_eq!((st, err_code(&v).as_str()), (409, "app_mismatch"), "{v}");
    let n = channel_posts(&env).len();
    assert_eq!(n, 2, "both asked Sew: {}", env.sew.log());

    // Another App's Space (K2 knows): refused before Sew.
    let (st, v) = create_channel(&f.pass, ask(theirs)).await;
    assert_eq!((st, err_code(&v).as_str()), (409, "app_mismatch"), "{v}");
    assert!(!v.to_string().contains(&other), "never names the other App: {v}");
    // Two Spaces of its own: name one.
    let (st, v) = create_channel(&f.pass, json!({"slug": "crew", "name": "Crew"})).await;
    assert_eq!((st, err_code(&v).as_str()), (400, "usage"), "{v}");
    // A guest's own pass, even of this App.
    skin::add_principal_for_app("ivy", None, &f.svc, None, "test").unwrap();
    skin::set_principal_password("ivy", Some("s3cret-horse-battery")).unwrap();
    let guest = skin::create_app_session_token("ivy", &f.svc).unwrap().raw;
    let (st, v) = create_channel(&guest, ask(unbound)).await;
    assert_eq!((st, err_code(&v).as_str()), (403, "app_pass_required"), "{v}");
    // Another App's own pass, on this App's Space.
    let theirs_pass =
        skin::link::mint_remote_pass(&other, "other", &["chat:channels".to_string()], &[uuid::Uuid::new_v4().to_string()], 3600).unwrap().raw;
    let (st, v) = create_channel(&theirs_pass, ask(unbound)).await;
    assert_eq!((st, err_code(&v).as_str()), (409, "app_mismatch"), "{v}");
    assert_eq!(channel_posts(&env).len(), n, "none of these reached Sew: {}", env.sew.log());
    env.sew.assert_clean();
}

// ── Sew 0.5.0: the managed-fields policy (k2_managed_avatar) ─────────

const POLICIES: &str = "/mgmt/v1/policies";

fn policy_calls(env: &Env, method: &str) -> usize {
    env.sew.calls().iter().filter(|c| c.path == POLICIES && c.method == method).count()
}

/// Sew 0.5.0 (option a, Sew 2026-10-28): once Sew reports
/// `k2_managed_avatar`, K2 reads the org policy and makes
/// `identity.managed_fields` = `[display_name, title, pronouns, avatar]`
/// (compared as a set; set once, again only when it drifts or on a new
/// install). An older Sew never sees the policy call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_fields_policy_follows_the_flag() {
    use serde_json::json;
    let k2_set = json!(["display_name", "title", "pronouns", "avatar"]);
    let env = setup().await;
    let p = skin::add_principal("ivy").unwrap();
    chat(&p.id);
    drain(&env).await;

    // An older Sew (no flag): never read, never set.
    worker::reconcile_once().await.expect("sweep without the flag");
    drain(&env).await;
    assert_eq!((policy_calls(&env, "GET"), policy_calls(&env, "POST")), (0, 0), "{}", env.sew.log());
    assert!(!hook::sew_policy_is_k2s().unwrap());

    // The flag: read (absent = Sew's default), then set once.
    env.sew.set_features(&["k2_managed_avatar"]);
    worker::reconcile_once().await.expect("sweep with the flag");
    drain(&env).await;
    assert_eq!(policy_calls(&env, "GET"), 1, "{}", env.sew.log());
    assert_eq!(policy_calls(&env, "POST"), 1, "{}", env.sew.log());
    assert_eq!(env.sew.policy("identity.managed_fields"), Some(k2_set.clone()));
    let set = env.sew.posts().into_iter().find(|c| c.path == POLICIES).unwrap();
    assert_eq!(set.body, json!({"changes": {"identity.managed_fields": k2_set.clone()}}));
    let row = rows().into_iter().find(|r| r.op == "policy_set").expect("a policy_set row (the audit trail)");
    assert_eq!((row.state.as_str(), row.actor.as_str(), row.cause.as_str()), ("done", "system:reconcile", "reconcile"));
    assert!(hook::sew_policy_is_k2s().unwrap(), "Sew's answer showed K2's set");

    // Idle sweep: read again, nothing sent.
    worker::reconcile_once().await.expect("idle sweep");
    drain(&env).await;
    assert_eq!((policy_calls(&env, "GET"), policy_calls(&env, "POST")), (2, 1));

    // The same set in another order is K2's: nothing sent.
    env.sew.set_policy("identity.managed_fields", Some(json!(["avatar", "pronouns", "title", "display_name"])));
    worker::reconcile_once().await.expect("sweep, reordered");
    drain(&env).await;
    assert_eq!(policy_calls(&env, "POST"), 1, "order is not significant");

    // An admin put Sew's default back (or narrowed it): K2 sets it again.
    for drift in [json!(["display_name", "title", "pronouns"]), json!(["display_name", "avatar"])] {
        let before = policy_calls(&env, "POST");
        env.sew.set_policy("identity.managed_fields", Some(drift.clone()));
        worker::reconcile_once().await.expect("sweep after drift");
        assert!(!hook::sew_policy_is_k2s().unwrap(), "{drift}: not K2's until Sew shows it");
        drain(&env).await;
        assert_eq!(policy_calls(&env, "POST"), before + 1, "{drift}");
        assert_eq!(env.sew.policy("identity.managed_fields"), Some(k2_set.clone()));
    }

    // A new install (its default shown explicitly): set again.
    env.sew.reinstall("246813579");
    env.sew.set_policy_default_shown(true);
    worker::reconcile_once().await.expect("sweep after reinstall");
    drain(&env).await;
    assert_eq!(policy_calls(&env, "POST"), 4, "{}", env.sew.log());
    assert_eq!(env.sew.policy("identity.managed_fields"), Some(k2_set));
    assert!(env.sew.member(&pext(&p.id)).is_some(), "re-provisioned");
    env.sew.assert_clean();
}

/// Sew 0.5.0's provision is a declarative upsert: an absent title or
/// pronouns clears them. So with the policy set, every provision K2 sends
/// to an existing member still carries the title and pronouns Sew holds (a
/// guest flip, a flip back, a reconcile repair), and nothing is cleared.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provisions_to_existing_members_always_carry_title_and_pronouns() {
    let env = setup().await;
    env.sew.set_features(&["k2_managed_avatar"]);
    worker::reconcile_once().await.expect("sweep with the flag");
    drain(&env).await;
    assert!(hook::sew_policy_is_k2s().unwrap());
    let p = skin::add_principal("joy").unwrap();
    let g = chat(&p.id);
    drain(&env).await;
    let ext = pext(&p.id);
    env.sew.set_sew_profile(&ext, None, Some("Founder"), Some("she/her"));
    let provisions = |env: &Env| -> Vec<serde_json::Value> {
        env.sew
            .posts()
            .into_iter()
            .filter(|c| c.path.ends_with("/provision") && c.body["external_id"] == ext.as_str())
            .map(|c| c.body)
            .collect()
    };
    let first = provisions(&env).len();

    skin::update_grant_scope(&g.id, "guest", "owner-token").unwrap();
    drain(&env).await;
    skin::update_grant_scope(&g.id, "member", "owner-token").unwrap();
    drain(&env).await;
    env.sew.with_members(|ms| ms.get_mut(&ext).unwrap().guest = true);
    worker::reconcile_once().await.expect("repair sweep");
    drain(&env).await;

    let sent = provisions(&env);
    assert_eq!(sent.len() - first, 3, "{}", env.sew.log());
    for b in &sent[first..] {
        assert_eq!(
            (b["profile"]["title"].as_str(), b["profile"]["pronouns"].as_str()),
            (Some("Founder"), Some("she/her")),
            "{b}"
        );
    }
    let m = env.sew.member(&ext).unwrap();
    assert!(!m.guest, "repaired");
    assert_eq!((m.title.as_deref(), m.pronouns.as_deref()), (Some("Founder"), Some("she/her")));
    assert_eq!(env.sew.profile_clears(), Vec::<String>::new(), "no provision cleared them");
    env.sew.assert_clean();
}

/// A failed policy read fails nothing else and sends nothing; the next
/// read decides.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_policy_read_sends_nothing() {
    let env = setup().await;
    env.sew.set_features(&["k2_managed_avatar"]);
    env.sew.set_policy_get_fails(true);
    let r = worker::reconcile_once().await.expect("sweep");
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    drain(&env).await;
    assert_eq!(policy_calls(&env, "POST"), 0, "{}", env.sew.log());
    assert!(!hook::sew_policy_is_k2s().unwrap());
    env.sew.set_policy_get_fails(false);
    worker::reconcile_once().await.expect("sweep");
    drain(&env).await;
    assert_eq!(policy_calls(&env, "POST"), 1, "{}", env.sew.log());
    assert!(hook::sew_policy_is_k2s().unwrap());
    env.sew.assert_clean();
}

// ── Sew 0.5.0: a removed App's Spaces are unbound in Sew (space_unbind) ─

const ORPHAN: &str = "48831765147549711";

/// An App with one user (`who`) who has chat, its Space bound on both sides.
async fn app_with_space(env: &Env, who: &str, space: &str) -> (String, String) {
    let svc = uuid::Uuid::new_v4().to_string();
    let u = skin::add_principal_for_app(who, None, &svc, None, "test").unwrap();
    chat(&u.id);
    drain(env).await;
    env.sew.add_app_space(space, &svc);
    hook::app_space_bind(&svc, space, "owner-token").unwrap();
    drain(env).await;
    assert!(env.sew.space(space).unwrap().members.contains(&pext(&u.id)), "{}", env.sew.log());
    (svc, u.id)
}

/// Sew 0.5.0: removing an App forgets its bindings (members stay) and
/// unbinds each Space in Sew: a read-only orphan, never released by K2.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_an_app_unbinds_its_spaces_in_sew() {
    let env = setup().await;
    env.sew.set_features(&["space_unbind"]);
    worker::reconcile_once().await.expect("sweep with the flag");
    let (svc, uid) = app_with_space(&env, "jon", ORPHAN).await;

    skin::app_chat::app_removed(&svc, "system:app-removed").unwrap();
    assert!(hook::app_spaces().unwrap().is_empty(), "K2 forgot the binding");
    drain(&env).await;
    let sp = env.sew.space(ORPHAN).unwrap();
    assert_eq!((sp.app_id.as_deref(), sp.orphaned_from.as_deref()), (None, Some(svc.as_str())), "{}", env.sew.log());
    assert!(sp.members.contains(&pext(&uid)), "members stay (read-only)");
    let un = env.sew.posts().into_iter().find(|c| c.path == "/mgmt/v1/spaces/unbind").unwrap();
    assert_eq!(un.body, serde_json::json!({"space_id": ORPHAN, "app_id": svc}));
    assert_eq!(un.actor.as_deref(), Some("system:app-removed"));
    let row = rows().into_iter().find(|r| r.op == "space_unbind").unwrap();
    assert_eq!(
        (row.state.as_str(), row.cause.as_str(), row.subject_key.as_str()),
        ("done", "app_removed", format!("unbind:{ORPHAN}").as_str())
    );
    let u = hook::app_space_unbinds().unwrap();
    assert_eq!(u.len(), 1);
    assert!(u[0].done_at.is_some());
    assert_eq!(u[0].orphaned_from.as_deref(), Some(svc.as_str()));

    // Later sweeps never write to the orphan (no adds, no removes) and
    // never unbind it again.
    let before = env.sew.posts().len();
    worker::reconcile_once().await.expect("sweep");
    drain(&env).await;
    let after: Vec<_> = env.sew.posts().into_iter().skip(before).filter(|c| c.path.contains("/spaces/")).map(|c| c.path).collect();
    assert!(after.is_empty(), "{after:?}");
    assert!(env.sew.calls().iter().all(|c| c.status != 422), "{}", env.sew.log());
    // Removing it again finds nothing to do.
    skin::app_chat::app_removed(&svc, "system:app-removed").unwrap();
    assert_eq!(hook::app_space_unbinds().unwrap().len(), 1);
    // assert_clean also proves K2 never sent `spaces/release`.
    env.sew.assert_clean();
}

/// An App removed while Sew lacks `space_unbind` (0.3.x): K2 keeps the
/// wish and unbinds after the upgrade. A Space bound again before then is
/// never orphaned.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_app_removed_before_the_upgrade_is_unbound_after_it() {
    let env = setup().await;
    let (svc, _) = app_with_space(&env, "kim", ORPHAN).await;
    let (svc2, _) = app_with_space(&env, "lou", SPACE).await;
    skin::app_chat::app_removed(&svc, "system:app-removed").unwrap();
    skin::app_chat::app_removed(&svc2, "system:app-removed").unwrap();
    worker::reconcile_once().await.expect("sweep without the flag");
    drain(&env).await;
    assert!(env.sew.posts().iter().all(|c| c.path != "/mgmt/v1/spaces/unbind"), "an older Sew never sees it");
    assert_eq!(env.sew.space(ORPHAN).unwrap().app_id.as_deref(), Some(svc.as_str()));
    let u = hook::app_space_unbinds().unwrap();
    assert_eq!(u.len(), 2);
    assert!(u.iter().all(|u| u.done_at.is_none()));
    // The second App's Space is bound again (the same App id) before the
    // upgrade: its unbind is dropped.
    hook::app_space_bind(&svc2, SPACE, "owner-token").unwrap();
    assert_eq!(hook::app_space_unbinds().unwrap().len(), 1);

    env.sew.set_features(&["space_unbind"]);
    worker::reconcile_once().await.expect("sweep after the upgrade");
    drain(&env).await;
    assert_eq!(env.sew.space(ORPHAN).unwrap().orphaned_from.as_deref(), Some(svc.as_str()), "{}", env.sew.log());
    assert_eq!(env.sew.space(SPACE).unwrap().app_id.as_deref(), Some(svc2.as_str()), "bound again: never orphaned");
    let unbinds: Vec<_> = env.sew.posts().into_iter().filter(|c| c.path == "/mgmt/v1/spaces/unbind").collect();
    assert_eq!(unbinds.len(), 1, "{}", env.sew.log());
    env.sew.assert_clean();
}
