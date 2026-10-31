//! App-governed Spaces (prd-app-governed-spaces-k2-v1 S4 + S5), end to end
//! against the fake Sew mgmt socket speaking sew-55 v2.4's shapes (I3, I4
//! single and bulk, I6, §3a), with Sew 0.6.0's flag `app_governed_spaces`.
//!
//! Tests drive `tool_hook_worker::{pass_once, reconcile_once}` directly.
//! Isolation: `TempHome::short()`; never the real `~/.k2` or `/run/sew`.
//! Every test fails loudly: no swallowed errors, no defaults in asserts.

#![cfg(unix)]

#[path = "support/fake_sew.rs"]
mod fake_sew;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use fake_sew::FakeSew;
use k2_core::deploy::manifest::{ChatBlock, ChatChannel};
use k2_core::skin::{self, SkinGrantWrite};
use k2_core::test_env::TempHome;
use k2_core::tool_hook::{self as hook, governed, sew_wire, CHAT_KIND};
use k2_daemon::tool_hook_worker::{self as worker, WorkerConfig};

const APP: &str = "7d4e2a10-1111-4222-8333-444455556666";
const SPACE: &str = "5101";
const CONNECT: &str = "k2:connect:3c4d5e6f-0000-4000-8000-000000000003";

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

async fn setup(flags: &[&str]) -> Env {
    let home = TempHome::short();
    hook::allow_target_off_linux_for_tests(true);
    let sew = FakeSew::start(&home.path().join("s")).await;
    sew.set_features(flags);
    hook::set_target(CHAT_KIND, true, Some(sew.socket.to_str().unwrap()), Some(sew.token_file.to_str().unwrap()), "owner-token")
        .expect("enable chat target");
    Env { _home: home, sew }
}

fn grant(w: SkinGrantWrite) -> skin::SkinGrant {
    skin::create_grant(&w).unwrap_or_else(|e| panic!("grant {w:?}: {e}"))
}

/// An App user with a login (role) on APP and a chat member grant.
fn user(name: &str, role: &str) -> skin::SkinPrincipal {
    let p = skin::add_principal(name).expect("principal");
    grant(SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: p.id.clone(),
        kind: "app".into(),
        target_id: APP.into(),
        role: Some(role.into()),
        scope: None,
        created_by: Some("owner-token".into()),
    });
    grant(SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: p.id.clone(),
        kind: "chat".into(),
        target_id: String::new(),
        role: None,
        scope: Some("member".into()),
        created_by: Some("owner-token".into()),
    });
    p
}

fn role(name: &str) {
    skin::create_role_for_app(name, &Default::default(), Some(APP)).unwrap_or_else(|e| panic!("role {name}: {e}"));
}

fn ch(slug: &str, private: bool, roles: &[&str]) -> ChatChannel {
    ChatChannel {
        slug: slug.into(),
        name: format!("{slug} room"),
        kind: "text".into(),
        private,
        user_limit: None,
        roles: roles.iter().map(|s| s.to_string()).collect(),
        rooms: vec![],
        agents: vec![],
        space: None,
    }
}

fn block() -> ChatBlock {
    ChatBlock {
        channels: vec![
            ChatChannel { kind: "voice".into(), user_limit: Some(25), ..ch("meeting-room", false, &[]) },
            ch("plans-desk", true, &["estimator", "office"]),
            ch("office-only", true, &["office"]),
        ],
        levels: [("office".to_string(), "admin".to_string())].into_iter().collect(),
    }
}

fn note_block(b: ChatBlock) {
    hook::note_app_chat_facts(&[governed::AppFacts { app_id: APP.into(), owner_workspace: None, block: Some(b), handles: BTreeMap::new() }])
        .expect("facts");
}

fn rows() -> Vec<hook::outbox::OutboxRow> {
    let mut r = hook::log(5000, None).expect("log");
    r.reverse();
    r
}

/// Sweep, then deliver until nothing is pending; again while a delivered
/// row (a switch, a channel op, a box user out) asks for another sweep.
async fn settle(env: &Env) {
    for _ in 0..15 {
        let _ = hook::take_reconcile_request();
        worker::reconcile_once().await.expect("sweep");
        for _ in 0..40 {
            if rows().iter().all(|r| r.state != "pending") {
                break;
            }
            hook::force_due_for_tests().unwrap();
            worker::pass_once(&cfg()).await.expect("pass");
        }
        if rows().iter().all(|r| r.state != "pending") && !hook::reconcile_requested() {
            return;
        }
    }
    panic!("never settled: {:?}\nfake log:\n{}", rows().iter().filter(|r| r.state == "pending").collect::<Vec<_>>(), env.sew.log());
}

/// Deliver only (no sweep) until nothing is pending.
async fn drain(env: &Env) {
    for _ in 0..40 {
        if rows().iter().all(|r| r.state != "pending") {
            return;
        }
        hook::force_due_for_tests().unwrap();
        worker::pass_once(&cfg()).await.expect("pass");
    }
    panic!("never drained: {}", env.sew.log());
}

fn pext(p: &skin::SkinPrincipal) -> String {
    format!("k2:principal:{}", p.id)
}

fn channel_id(env: &Env, slug: &str) -> String {
    env.sew.channel(SPACE, slug).unwrap_or_else(|| panic!("no channel {slug} in Sew")).id.to_string()
}

fn sync_posts(env: &Env) -> usize {
    env.sew.posts().iter().filter(|c| c.path.contains("/members/") && c.path.ends_with("channels")).count()
}

/// A Space set up on Sew 0.5.0: bound to the App, owned by the box owner,
/// with a Connect admin in it; two App users with roles.
async fn bound_space(env: &Env) -> (skin::SkinPrincipal, skin::SkinPrincipal) {
    hook::put_chat_key_for_tests(APP, "moderator", 500).unwrap();
    role("estimator");
    role("office");
    let est = user("estella", "estimator");
    let off = user("oscar", "office");
    note_block(block());
    env.sew.add_app_space(SPACE, APP);
    env.sew.set_space_owner(SPACE, "k2:owner");
    env.sew.add_space_member(SPACE, CONNECT);
    hook::app_space_bind(APP, SPACE, "owner-token").expect("bind");
    settle(env).await;
    (est, off)
}

/// The migration (§8) and the sync (§5), end to end: the switch clears box
/// users first, the service owner owns the Space, the manifest channels
/// are ensured, and each App user's private channels follow their role.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bound_space_switches_to_app_and_people_get_their_roles_channels() {
    let env = setup(&[governed_flag(), "space_unbind"]).await;
    let (est, off) = bound_space(&env).await;
    assert_eq!(sync_posts(&env), 0, "nothing synced while Sew governs the Space");
    assert!(hook::set_space_governance(APP, SPACE, "app", "owner-token").expect("switch"));
    settle(&env).await;

    let sp = env.sew.space(SPACE).expect("space");
    assert_eq!(sp.governance.as_deref(), Some("app"));
    assert_eq!(sp.owner.as_deref(), Some(format!("app:{APP}").as_str()), "the App's service owner owns it");
    assert!(!sp.members.contains(CONNECT), "the Connect admin left before the switch");
    assert!(!sp.members.contains("k2:owner"), "the box owner left with the switch");
    let removes: Vec<_> = env.sew.posts().into_iter().filter(|c| c.path == sew_wire::MGMT_SPACE_MEMBERS_REMOVE).collect();
    assert_eq!(removes.len(), 1, "exactly the listed box user: {removes:?}");
    assert_eq!(removes[0].body["external_id"], CONNECT);

    let (e_ids, e_level) = env.sew.member_channels(SPACE, &pext(&est));
    assert_eq!(e_ids, BTreeSet::from([channel_id(&env, "plans-desk")]));
    assert_eq!(e_level.as_deref(), Some("member"));
    let (o_ids, o_level) = env.sew.member_channels(SPACE, &pext(&off));
    assert_eq!(o_ids, BTreeSet::from([channel_id(&env, "plans-desk"), channel_id(&env, "office-only")]));
    assert_eq!(o_level.as_deref(), Some("moderator"), "office = admin in the manifest, clamped to the key's moderator");
    assert_eq!(env.sew.bulk_calls(), 1, "both people in one I4 bulk call");
    assert!(env.sew.channel(SPACE, "meeting-room").is_some_and(|c| !c.private && c.kind == "voice"), "the public voice room is ensured");

    // Idempotent: another sweep sends nothing.
    let before = sync_posts(&env);
    settle(&env).await;
    assert_eq!(sync_posts(&env), before, "a repeat sends nothing");
    // A role change (k2 app grant role) sends that one person's list (I4 single).
    // Her login on APP: add_principal also gives her a host grant (kind=app,
    // target host), and the two can share a created_at second.
    let g = skin::list_grants()
        .expect("grants")
        .into_iter()
        .find(|g| g.kind == "app" && g.subject_id == est.id && g.target_id == APP)
        .expect("estella's login");
    assert!(skin::set_app_grant_role(&g.id, "office").expect("role change").1);
    assert!(hook::reconcile_requested(), "a role change asks for a sweep");
    settle(&env).await;
    assert_eq!(sync_posts(&env), before + 1, "one item, sent alone");
    assert_eq!(env.sew.member_channels(SPACE, &pext(&est)).0.len(), 2);
    let v = hook::governed_view(APP).expect("view");
    assert_eq!(v.spaces[0].leaving.len(), 1);
    assert!(v.spaces[0].people.iter().all(|p| p.state == governed::SYNCED), "{:?}", v.spaces[0].people);
    env.sew.assert_clean();
}

fn governed_flag() -> &'static str {
    hook::features::APP_GOVERNED_SPACES
}

/// Bans win; a deprovisioned person is skipped and never reactivated.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bans_win_and_a_deprovisioned_person_is_never_reactivated() {
    let env = setup(&[governed_flag()]).await;
    let (est, off) = bound_space(&env).await;
    hook::set_space_governance(APP, SPACE, "app", "owner-token").unwrap();
    settle(&env).await;
    let est_before = env.sew.member_channels(SPACE, &pext(&est)).0;
    // The App changes both people's lists; the sweep queues their syncs...
    let lists = vec![
        governed::AppList { principal_id: est.id.clone(), space_id: SPACE.into(), slugs: vec!["office-only".into()], level: None },
        governed::AppList { principal_id: off.id.clone(), space_id: SPACE.into(), slugs: vec![], level: Some("member".into()) },
    ];
    assert_eq!(hook::set_app_member_lists(APP, &lists, "app:test").unwrap(), 2);
    worker::reconcile_once().await.unwrap();
    // ...then, in Sew, a person bans estella and deactivates oscar.
    env.sew.ban_in_space(SPACE, &pext(&est));
    env.sew.with_members(|m| {
        m.get_mut(&pext(&off)).expect("oscar in Sew").deactivated = true;
    });
    drain(&env).await;
    let v = hook::governed_view(APP).unwrap();
    let state = |ext: &str| v.spaces[0].people.iter().find(|p| p.external_id == ext).map(|p| (p.state.clone(), p.why.clone())).expect("person");
    assert_eq!(state(&pext(&est)), (governed::HELD.into(), Some(governed::WHY_BANNED.into())));
    assert_eq!(state(&pext(&off)), (governed::HELD.into(), Some(governed::WHY_DEPROVISIONED.into())));
    assert_eq!(env.sew.member_channels(SPACE, &pext(&est)).0, est_before, "bans win: no channel added");
    assert!(env.sew.member(&pext(&off)).expect("member").deactivated, "the sync never reactivates");
    // Held items are not re-sent by the next sweep.
    let syncs_before = sync_posts(&env);
    settle(&env).await;
    assert_eq!(sync_posts(&env), syncs_before, "held items are not re-sent");
    env.sew.assert_clean();
}

/// 422 member_not_provisioned: K2 provisions the person again, then syncs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_person_missing_in_sew_is_provisioned_again_then_synced() {
    let env = setup(&[governed_flag()]).await;
    let (est, _off) = bound_space(&env).await;
    hook::set_space_governance(APP, SPACE, "app", "owner-token").unwrap();
    settle(&env).await;
    let provisions = |env: &Env| env.sew.posts().iter().filter(|c| c.path == sew_wire::MGMT_PROVISION && c.body["external_id"] == pext(&est)).count();
    assert_eq!(provisions(&env), 1);
    // The App gives her office-only too; the sweep queues her sync...
    let lists = vec![governed::AppList { principal_id: est.id.clone(), space_id: SPACE.into(), slugs: vec!["office-only".into()], level: None }];
    assert_eq!(hook::set_app_member_lists(APP, &lists, "app:test").unwrap(), 1);
    worker::reconcile_once().await.unwrap();
    // ...and she is removed in Sew outside K2 before it goes.
    env.sew.with_members(|m| {
        m.remove(&pext(&est)).expect("estella in Sew");
    });
    // Delivery only (no sweep, which would re-provision her on its own).
    drain(&env).await;
    let refused: Vec<_> = env.sew.calls().into_iter().filter(|c| c.status == 422 && c.path.contains("/members/")).collect();
    assert_eq!(refused.len(), 1, "member_not_provisioned once: {}", env.sew.log());
    assert_eq!(provisions(&env), 2, "provisioned again, by K2 (the sync makes nobody)");
    assert!(env.sew.member(&pext(&est)).is_some_and(|m| !m.deactivated));
    assert_eq!(env.sew.member_channels(SPACE, &pext(&est)).0, BTreeSet::from([channel_id(&env, "plans-desk"), channel_id(&env, "office-only")]));
    env.sew.assert_clean();
}

/// A person with many channels: one full list; over 1,000 changes Sew
/// answers `queued`, and the next sweep reads it back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_large_change_is_queued_by_sew_and_read_back_next_sweep() {
    let env = setup(&[governed_flag()]).await;
    let (est, _off) = bound_space(&env).await;
    hook::set_space_governance(APP, SPACE, "app", "owner-token").unwrap();
    settle(&env).await;
    // The App made 1,200 private channels and gives estella all of them.
    let made = env.sew.add_channels(SPACE, 1200);
    let req = sew_wire::ChannelsBulkRequest {
        space_id: SPACE.into(),
        app_id: APP.into(),
        ops: made
            .iter()
            .map(|(slug, _)| sew_wire::ChannelOp {
                op: "ensure".into(),
                slug: Some(slug.clone()),
                channel_id: None,
                name: Some(slug.clone()),
                kind: Some("text".into()),
                private: Some(true),
                user_limit: None,
            })
            .collect(),
    };
    let resp = sew_wire::ChannelsBulkResponse {
        results: made
            .iter()
            .map(|(slug, id)| sew_wire::ChannelOpResult { op: "ensure".into(), slug: Some(slug.clone()), channel_id: Some(id.clone()), name: Some(slug.clone()), created: true, error: None })
            .collect(),
    };
    hook::note_app_channel_ops(SPACE, APP, &req, &resp).expect("note");
    let lists = vec![governed::AppList { principal_id: est.id.clone(), space_id: SPACE.into(), slugs: made.iter().map(|(s, _)| s.clone()).collect(), level: None }];
    assert_eq!(hook::set_app_member_lists(APP, &lists, "app:test").unwrap(), 1);
    worker::reconcile_once().await.unwrap();
    hook::force_due_for_tests().unwrap();
    worker::pass_once(&cfg()).await.unwrap();
    let v = hook::governed_view(APP).unwrap();
    let p = v.spaces[0].people.iter().find(|p| p.external_id == pext(&est)).expect("estella");
    assert_eq!(p.state, governed::QUEUED, "over 1,000 changes: Sew ran it as a job");
    assert_eq!(p.channels.len(), 1201);
    settle(&env).await;
    let v = hook::governed_view(APP).unwrap();
    assert_eq!(v.spaces[0].people.iter().find(|p| p.external_id == pext(&est)).unwrap().state, governed::SYNCED, "read back");
    assert_eq!(env.sew.member_channels(SPACE, &pext(&est)).0.len(), 1201);
    env.sew.assert_clean();
}

/// Sew 0.6.0's exact I4 shapes, end to end (bulk items carry
/// `{external_id, status, answer}`): a Space at the App's member limit
/// answers `skipped: quota_exceeded`, which K2 holds with that reason and
/// doesn't re-send on a sweep; once the limit is raised, the person's next
/// list goes and they join. A channel made public in Sew comes back in
/// `unknown_channel_ids` while the rest of the list (and the level)
/// applies; K2 holds the person and says which channel.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_space_holds_people_and_a_public_channel_is_unknown_while_the_rest_applies() {
    let env = setup(&[governed_flag()]).await;
    let (est, off) = bound_space(&env).await;
    hook::set_space_governance(APP, SPACE, "app", "owner-token").unwrap();
    settle(&env).await;
    let person = |ext: &str| {
        let v = hook::governed_view(APP).expect("view");
        v.spaces[0].people.iter().find(|p| p.external_id == ext).map(|p| (p.state.clone(), p.why.clone())).unwrap_or_else(|| panic!("{ext} in the view"))
    };

    // Oscar left the Space in Sew, and it is full for this App now. Both
    // people's lists change in one sweep: one bulk call; estella (still in)
    // syncs, oscar is skipped.
    env.sew.remove_from_space(SPACE, &pext(&off));
    let full = env.sew.space(SPACE).expect("space").members.len();
    env.sew.set_member_quota(APP, Some(full));
    let lists = vec![
        governed::AppList { principal_id: est.id.clone(), space_id: SPACE.into(), slugs: vec!["office-only".into()], level: None },
        governed::AppList { principal_id: off.id.clone(), space_id: SPACE.into(), slugs: vec![], level: Some("member".into()) },
    ];
    assert_eq!(hook::set_app_member_lists(APP, &lists, "app:test").unwrap(), 2);
    let bulk_before = env.sew.bulk_calls();
    worker::reconcile_once().await.unwrap();
    drain(&env).await;
    assert_eq!(env.sew.bulk_calls(), bulk_before + 1, "both in one bulk call: {}", env.sew.log());
    assert_eq!(person(&pext(&off)), (governed::HELD.to_string(), Some(governed::WHY_QUOTA.to_string())));
    assert_eq!(person(&pext(&est)).0, governed::SYNCED, "the bulk item's nested answer is read");
    assert_eq!(
        env.sew.member_channels(SPACE, &pext(&est)).0,
        BTreeSet::from([channel_id(&env, "plans-desk"), channel_id(&env, "office-only")])
    );
    assert!(!env.sew.space(SPACE).unwrap().members.contains(&pext(&off)), "skipped: not joined");
    // A sweep with nothing changed doesn't send oscar again.
    let syncs = sync_posts(&env);
    worker::reconcile_once().await.unwrap();
    drain(&env).await;
    assert_eq!(sync_posts(&env), syncs, "a quota hold waits");
    // The limit goes; oscar's list changes: he joins.
    env.sew.set_member_quota(APP, None);
    let lists = vec![governed::AppList { principal_id: off.id.clone(), space_id: SPACE.into(), slugs: vec![], level: Some("moderator".into()) }];
    assert_eq!(hook::set_app_member_lists(APP, &lists, "app:test").unwrap(), 1);
    worker::reconcile_once().await.unwrap();
    drain(&env).await;
    assert_eq!(person(&pext(&off)).0, governed::SYNCED, "{}", env.sew.log());
    assert!(env.sew.space(SPACE).unwrap().members.contains(&pext(&off)));

    // A moderator makes office-only public in Sew. Estella's next list
    // still names it (K2's manifest says private): Sew reports that id as
    // unknown and applies the rest, her new level included.
    env.sew.set_channel_private(SPACE, "office-only", false);
    let office_only = channel_id(&env, "office-only");
    let lists = vec![governed::AppList { principal_id: est.id.clone(), space_id: SPACE.into(), slugs: vec!["office-only".into()], level: Some("moderator".into()) }];
    assert_eq!(hook::set_app_member_lists(APP, &lists, "app:test").unwrap(), 1);
    worker::reconcile_once().await.unwrap();
    drain(&env).await;
    let (state, why) = person(&pext(&est));
    assert_eq!(state, governed::HELD);
    assert_eq!(why.as_deref(), Some(format!("unknown channels: {office_only}").as_str()));
    let (ids, level) = env.sew.member_channels(SPACE, &pext(&est));
    assert!(ids.contains(&channel_id(&env, "plans-desk")), "the rest of the list applied: {ids:?}");
    assert_eq!(level.as_deref(), Some("moderator"), "the level applied too");
    env.sew.assert_clean();
}

/// Without Sew's flag nothing is switched, synced or ensured.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_the_flag_nothing_is_sent() {
    let env = setup(&["space_unbind"]).await;
    let (_est, _off) = bound_space(&env).await;
    assert_eq!(hook::set_space_governance(APP, SPACE, "app", "owner-token"), Err(hook::GovernanceRefusal::FeatureMissing));
    settle(&env).await;
    assert_eq!(sync_posts(&env), 0);
    assert!(env.sew.posts().iter().all(|c| !c.path.ends_with("/settings") || c.path.starts_with("/mgmt/v1/apps/")), "{}", env.sew.log());
    assert_eq!(env.sew.space(SPACE).unwrap().governance, None);
    env.sew.assert_clean();
}

/// Uninstall: the App's Spaces are unbound (Sew resets governance), then
/// its service owner is deprovisioned.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uninstall_unbinds_then_deprovisions_the_service_owner() {
    let env = setup(&[governed_flag(), "space_unbind"]).await;
    let (_est, _off) = bound_space(&env).await;
    hook::set_space_governance(APP, SPACE, "app", "owner-token").unwrap();
    settle(&env).await;
    let svc = format!("app:{APP}");
    assert!(env.sew.member(&svc).is_none(), "the fake makes the service owner only on an App-owned create");
    // An App-owned Space made the service owner in Sew.
    env.sew.with_members(|m| {
        let mut owner = m.values().next().expect("someone").clone();
        owner.external_id = svc.clone();
        owner.member_id = 99_999;
        owner.username = "app-service".into();
        m.insert(svc.clone(), owner);
    });
    hook::app_spaces_app_removed(APP, "owner-token").expect("removed");
    assert_eq!(hook::space_governance(SPACE).unwrap(), (None, None), "K2 forgets its wish with the unbind");
    settle(&env).await;
    let posts = env.sew.posts();
    let unbind = posts.iter().position(|c| c.path == sew_wire::MGMT_SPACES_UNBIND).expect("unbind sent");
    let deprov = posts.iter().position(|c| c.path == sew_wire::MGMT_DEPROVISION && c.body["external_id"] == svc).expect("service owner deprovisioned");
    assert!(unbind < deprov, "after the unbind");
    assert!(env.sew.member(&svc).unwrap().deactivated);
    env.sew.assert_clean();
}

/// The App's own call names its own users only (no oracle) and never an
/// agent login.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_apps_own_call_resolves_only_its_own_users() {
    let env = setup(&[governed_flag()]).await;
    let (est, _off) = bound_space(&env).await;
    assert_eq!(hook::app_user_for_call(APP, "estella").unwrap(), Ok(est.id.clone()));
    assert_eq!(hook::app_user_for_call(APP, "nobody").unwrap(), Err("not_found"));
    let other = skin::add_principal("outsider").unwrap();
    assert_eq!(hook::app_user_for_call(APP, &other.username).unwrap(), Err("not_found"), "not this App's user");
    assert_eq!(hook::governed_space_for_app(APP, None).unwrap(), Ok(SPACE.to_string()));
    let mismatch = hook::governed_space_for_app("8e5f3b21-2222-4333-8444-555566667777", Some(SPACE)).unwrap();
    assert!(matches!(mismatch, Err((409, "chat_key_required", _))), "{mismatch:?}");
    env.sew.assert_clean();
}
