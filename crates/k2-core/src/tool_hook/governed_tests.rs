//! App-governed Spaces (S4 + S5), K2's half: the sync's rules, one test
//! per rule. Each fails loudly (no defaults in assertions, no skips).

use std::collections::BTreeMap;

use serde_json::json;

use super::*;
use crate::deploy::manifest::{ChatBlock, ChatChannel};
use crate::skin::{self, SkinGrantWrite};
use crate::tool_hook::{self as hook, app_space, outbox, CHAT_KIND};

const APP: &str = "7d4e2a10-1111-4222-8333-444455556666";
const OTHER_APP: &str = "8e5f3b21-2222-4333-8444-555566667777";
const SPACE: &str = "spc_meeting1";
const DESK_WS: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
const OWN_WS: &str = "1a2b3c4d-0000-4000-8000-000000000001";
const STRANGER_WS: &str = "2b3c4d5e-0000-4000-8000-000000000002";

struct Env {
    _home: crate::test_env::TempHome,
}

fn setup() -> Env {
    let home = crate::test_env::TempHome::new();
    hook::allow_target_off_linux_for_tests(true);
    let _ = hook::take_reconcile_request();
    hook::set_target(CHAT_KIND, true, Some("/tmp/k2-test-sew.sock"), Some("/tmp/k2-test-sew.token"), "owner-token")
        .expect("enable chat target");
    hook::note_features_for_tests(&[features::APP_GOVERNED_SPACES]).expect("flag");
    Env { _home: home }
}

fn sql(q: &str, p: &[&dyn rusqlite::ToSql]) {
    skin::with_skin_conn(|c| c.execute(q, p).map(|_| ()).map_err(|e| e.to_string())).unwrap_or_else(|e| panic!("{q}: {e}"));
}

/// S1's table (its DDL per PRD §2.2) and one enabled key.
fn key(app: &str, max_level: &str, max_members: i64) {
    sql(
        "CREATE TABLE IF NOT EXISTS app_chat_keys (service_id TEXT PRIMARY KEY, enabled INTEGER NOT NULL, caps TEXT NOT NULL DEFAULT '[]',
            max_level TEXT NOT NULL DEFAULT 'moderator', uploads INTEGER NOT NULL DEFAULT 1, max_members INTEGER NOT NULL DEFAULT 500,
            granted_by TEXT, granted_at INTEGER, revoked_at INTEGER)",
        &[],
    );
    sql(
        "INSERT OR REPLACE INTO app_chat_keys (service_id, enabled, max_level, max_members) VALUES (?1, 1, ?2, ?3)",
        &[&app, &max_level, &max_members],
    );
}

fn role(name: &str, app: &str, rooms: &[&str]) {
    let policy = skin::thread_only_room_policy(&rooms.iter().map(|r| r.to_string()).collect::<Vec<_>>());
    skin::create_role_for_app(name, &policy, Some(app)).unwrap_or_else(|e| panic!("role {name}: {e}"));
}

fn grant(w: SkinGrantWrite) -> skin::SkinGrant {
    skin::create_grant(&w).unwrap_or_else(|e| panic!("grant {w:?}: {e}"))
}

/// An App user with a login on `app` (role optional) and their chat grant.
fn user(name: &str, app: &str, role: Option<&str>) -> skin::SkinPrincipal {
    let p = skin::add_principal(name).expect("principal");
    grant(SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: p.id.clone(),
        kind: "app".into(),
        target_id: app.into(),
        role: role.map(str::to_string),
        // kind=app takes a role or a rooms scope, never both.
        scope: if role.is_some() { None } else { Some("[]".into()) },
        created_by: Some("owner-token".into()),
    });
    chat(&p.id);
    p
}

fn chat(principal: &str) -> skin::SkinGrant {
    grant(SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: principal.into(),
        kind: "chat".into(),
        target_id: String::new(),
        role: None,
        scope: Some("member".into()),
        created_by: Some("owner-token".into()),
    })
}

fn ch(slug: &str, private: bool, roles: &[&str], rooms: &[&str], agents: &[&str]) -> ChatChannel {
    ChatChannel {
        slug: slug.into(),
        name: format!("{slug} name"),
        kind: "text".into(),
        private,
        user_limit: None,
        roles: roles.iter().map(|s| s.to_string()).collect(),
        rooms: rooms.iter().map(|s| s.to_string()).collect(),
        agents: agents.iter().map(|s| s.to_string()).collect(),
        space: None,
    }
}

fn facts(block: ChatBlock) {
    let handles: BTreeMap<String, String> =
        [("plans-desk", DESK_WS), ("own", OWN_WS), ("stranger", STRANGER_WS)].into_iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
    hook::note_app_chat_facts(&[AppFacts { app_id: APP.into(), owner_workspace: Some(OWN_WS.into()), block: Some(block), handles }]).expect("facts");
}

fn bind_and_govern(app: &str) {
    skin::with_skin_conn(|c| app_space::bind(c, app, SPACE, "owner-token", 1).map(|_| ())).expect("bind");
    hook::set_space_governance(app, SPACE, "app", "owner-token").expect("governance");
    // Sew confirmed it (what the worker records on a 2xx).
    sql("UPDATE tool_sew_settings SET sent_body = body WHERE subject = ?1", &[&settings_subject(SPACE)]);
}

fn rows() -> Vec<outbox::OutboxRow> {
    let mut r = hook::log(5000, None).expect("log");
    r.reverse();
    r
}

fn open_rows(op: &str) -> Vec<outbox::OutboxRow> {
    rows().into_iter().filter(|r| r.op == op && r.state == outbox::STATE_PENDING).collect()
}

/// One sweep's planning (channels, then people), enqueued.
fn plan() -> Summary {
    let now = outbox::now_secs();
    skin::with_skin_conn(|c| {
        let tx = c.unchecked_transaction().map_err(|e| e.to_string())?;
        let busy = outbox::busy_subjects(&tx, CHAT_KIND)?;
        let mut ops = plan_channels(&tx, &busy, now)?;
        let (m, s) = plan_members(&tx, &busy, now)?;
        ops.extend(m);
        outbox::enqueue(&tx, &ops, hook::ACTOR_RECONCILE, now)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(s)
    })
    .expect("plan")
}

/// Sew answers every open `channels_bulk` row: each op gets `chan_<slug>`.
fn answer_channels() {
    for r in open_rows("channels_bulk") {
        let req: ChannelsBulkRequest = serde_json::from_str(&r.payload_raw).expect("channels body");
        let results: Vec<_> = req
            .ops
            .iter()
            .map(|o| json!({"op": o.op, "slug": o.slug, "channel_id": format!("chan_{}", o.slug.clone().expect("slug")), "created": true}))
            .collect();
        hook::record_done(&r.id, 200, &json!({ "results": results }).to_string()).expect("done");
    }
}

/// Deliver every open provision (the person's member exists in Sew).
fn provision_all() {
    for r in open_rows("provision") {
        let body = json!({ "member_id": format!("m{}", r.seq), "created": true }).to_string();
        hook::record_done(&r.id, 200, &body).expect("provision done");
    }
}

fn sync_rows() -> Vec<outbox::OutboxRow> {
    open_rows("space_member_channels")
}

fn sync_body(ext: &str) -> MemberChannelsRequest {
    let r = sync_rows().into_iter().find(|r| r.subject_key == chan_subject(SPACE, &ExternalId::parse(ext).unwrap())).unwrap_or_else(|| {
        panic!("no open sync row for {ext}: {:?}", sync_rows().iter().map(|r| &r.subject_key).collect::<Vec<_>>())
    });
    serde_json::from_str(&r.payload_raw).expect("sync body")
}

fn ledger(ext: &str) -> (String, Option<String>) {
    skin::with_skin_conn(|c| {
        c.query_row(
            "SELECT state, why FROM tool_app_space_member_channels WHERE space_id = ?1 AND external_id = ?2",
            rusqlite::params![SPACE, ext],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| format!("ledger {ext}: {e}"))
    })
    .expect("ledger")
}

fn pext(p: &skin::SkinPrincipal) -> String {
    format!("k2:principal:{}", p.id)
}

fn standard_block() -> ChatBlock {
    ChatBlock {
        channels: vec![
            ChatChannel { kind: "voice".into(), user_limit: Some(25), ..ch("meeting-room", false, &[], &[], &[]) },
            ch("plans-desk", true, &["estimator", "office"], &[], &["plans-desk"]),
            ch("office-only", true, &["office"], &[], &[]),
            ch("by-room", true, &[], &["plans-desk"], &[]),
        ],
        levels: [("office".to_string(), "admin".to_string())].into_iter().collect(),
    }
}

/// A governed Space with channels ensured and every provision delivered.
fn ready() -> (skin::SkinPrincipal, skin::SkinPrincipal) {
    key(APP, "moderator", 500);
    role("estimator", APP, &[]);
    role("office", APP, &[DESK_WS]);
    let est = user("estella", APP, Some("estimator"));
    let off = user("oscar", APP, Some("office"));
    facts(standard_block());
    bind_and_govern(APP);
    provision_all();
    plan();
    answer_channels();
    (est, off)
}

#[test]
fn roles_and_rooms_decide_private_channels_and_levels_are_clamped_to_the_key() {
    let _env = setup();
    let (est, off) = ready();
    let s = plan();
    assert_eq!(s.queued, 2, "{s:?}");
    let e = sync_body(&pext(&est));
    assert_eq!(e.app_id, APP);
    assert_eq!(e.channel_ids, vec!["chan_plans-desk".to_string()], "estimator: plans-desk only (public voice is never listed)");
    assert_eq!(e.level, "member");
    let o = sync_body(&pext(&off));
    assert_eq!(
        o.channel_ids,
        vec!["chan_by-room".to_string(), "chan_office-only".into(), "chan_plans-desk".into()],
        "office: its roles' channels plus by-room through its room line"
    );
    assert_eq!(o.level, "moderator", "levels.office = admin, clamped to the key's moderator ceiling");
}

#[test]
fn a_repeat_plan_sends_nothing_and_a_role_change_sends_one_new_list() {
    let _env = setup();
    let (est, _off) = ready();
    plan();
    for r in sync_rows() {
        hook::record_done(&r.id, 200, r#"{"added":[],"removed":[],"unchanged":[],"level":"member","unknown_channel_ids":[],"skipped":null}"#).unwrap();
    }
    assert_eq!(ledger(&pext(&est)).0, SYNCED);
    let s = plan();
    assert_eq!(s.queued, 0, "idempotent: {s:?}");
    assert!(sync_rows().is_empty());
    // estella moves to office.
    sql("UPDATE grants SET role_id = (SELECT id FROM roles WHERE name = 'office') WHERE kind = 'app' AND subject_id = ?1", &[&est.id]);
    let s = plan();
    assert_eq!(s.queued, 1, "{s:?}");
    assert_eq!(sync_body(&pext(&est)).channel_ids.len(), 3);
}

#[test]
fn the_sync_row_waits_behind_the_persons_own_provision() {
    let _env = setup();
    key(APP, "moderator", 500);
    role("estimator", APP, &[]);
    facts(standard_block());
    bind_and_govern(APP);
    plan();
    answer_channels();
    let late = user("lateo", APP, Some("estimator"));
    plan();
    let sync = sync_rows().into_iter().find(|r| r.subject_key.ends_with(&pext(&late))).expect("sync row");
    assert_eq!(sync.after_subject.as_deref(), Some(pext(&late).as_str()), "ordered behind the person's provision rows");
    let due = |id: &str| {
        skin::with_skin_conn(|c| outbox::due_heads(c, CHAT_KIND, i64::MAX, 1000)).unwrap().iter().any(|r| r.id == id)
    };
    assert!(!due(&sync.id), "the sync is not due while the provision is open");
    provision_all();
    assert!(due(&sync.id), "due once the provision is delivered");
}

#[test]
fn box_users_agent_logins_and_tied_people_are_never_synced() {
    let _env = setup();
    let (_est, _off) = ready();
    let agentish = user("agentina", APP, Some("estimator"));
    sql("UPDATE principals SET agent_workspace = ?1 WHERE id = ?2", &[&OWN_WS, &agentish.id]);
    let tied = user("tiedy", APP, Some("estimator"));
    sql(
        "INSERT INTO principal_agent_ties (principal_id, workspace_id, host_ok, reasons, set_by, set_at) VALUES (?1, ?2, 0, '[]', 'agent:x', 1)",
        &[&tied.id, &OWN_WS],
    );
    provision_all();
    plan();
    let subjects: Vec<String> = sync_rows().into_iter().map(|r| r.subject_key).collect();
    assert!(!subjects.iter().any(|s| s.contains(&agentish.id)), "agent login synced: {subjects:?}");
    assert!(!subjects.iter().any(|s| s.contains(&tied.id)), "tied person synced: {subjects:?}");
    assert!(!subjects.iter().any(|s| s.contains("k2:owner") || s.contains("k2:connect:")), "box user synced: {subjects:?}");
    // K2 refuses adding a box user to a Space it governs.
    assert!(hook::governed_refuses_box_user(SPACE, &ExternalId::K2Owner).unwrap());
    let connect = ExternalId::parse("k2:connect:3c4d5e6f-0000-4000-8000-000000000003").unwrap();
    assert!(hook::governed_refuses_box_user(SPACE, &connect).unwrap());
    assert!(!hook::governed_refuses_box_user(SPACE, &ExternalId::parse(&pext(&tied)).unwrap()).unwrap());
}

#[test]
fn agents_join_private_text_channels_at_member_once_provisioned_and_only_allowed_ones() {
    let _env = setup();
    key(APP, "admin", 500);
    role("office", APP, &[DESK_WS]);
    let block = ChatBlock {
        channels: vec![ch("plans-desk", true, &["office"], &[], &["plans-desk", "own", "stranger"])],
        levels: BTreeMap::new(),
    };
    facts(block);
    bind_and_govern(APP);
    plan();
    answer_channels();
    let s = plan();
    let w = s.waiting.get(SPACE).expect("waiting");
    assert_eq!(w.refused_agents, vec!["stranger".to_string()], "a workspace that is neither the App's nor a room of its roles");
    assert_eq!(w.agents.len(), 2, "not provisioned in chat yet: {w:?}");
    assert!(sync_rows().iter().all(|r| !r.subject_key.contains("k2:workspace:")));
    hook::map_member_for_tests(&format!("k2:workspace:{DESK_WS}"), "m-desk", "agent", "desk", "active").unwrap();
    hook::map_member_for_tests(&format!("k2:workspace:{OWN_WS}"), "m-own", "agent", "own", "active").unwrap();
    plan();
    for ws in [DESK_WS, OWN_WS] {
        let b = sync_body(&format!("k2:workspace:{ws}"));
        assert_eq!(b.channel_ids, vec!["chan_plans-desk".to_string()]);
        assert_eq!(b.level, "member", "agents are always member");
    }
    assert!(!sync_rows().iter().any(|r| r.subject_key.contains(STRANGER_WS)));
    // Voice channels never list agents (validate refuses it).
    let bad = ChatBlock { channels: vec![ChatChannel { kind: "voice".into(), ..ch("call", true, &[], &[], &["own"]) }], levels: BTreeMap::new() };
    assert!(bad.problems().iter().any(|(f, _)| f.ends_with("agents")), "{:?}", bad.problems());
}

#[test]
fn the_apps_own_list_is_added_to_the_role_list_and_its_level_wins_clamped() {
    let _env = setup();
    let (est, _off) = ready();
    hook::set_app_member_lists(
        APP,
        &[AppList { principal_id: est.id.clone(), space_id: SPACE.into(), slugs: vec!["office-only".into(), "meeting-room".into(), "nope".into()], level: Some("admin".into()) }],
        "app:test",
    )
    .unwrap();
    plan();
    let b = sync_body(&pext(&est));
    assert_eq!(b.channel_ids, vec!["chan_office-only".to_string(), "chan_plans-desk".into()], "role ∪ App list; public and unknown slugs are not listed");
    assert_eq!(b.level, "moderator", "the App's level, clamped to the key");
}

#[test]
fn bans_win_and_stay_held_until_cleared() {
    let _env = setup();
    let (est, _off) = ready();
    plan();
    let r = sync_rows().into_iter().find(|r| r.subject_key.ends_with(&pext(&est))).unwrap();
    hook::record_held(&r.id, 200, r#"{"skipped":"banned"}"#, WHY_BANNED).unwrap();
    assert_eq!(ledger(&pext(&est)), (HELD.to_string(), Some(WHY_BANNED.to_string())));
    // A role change does not re-send a banned person.
    sql("UPDATE grants SET role_id = (SELECT id FROM roles WHERE name = 'office') WHERE kind = 'app' AND subject_id = ?1", &[&est.id]);
    plan();
    assert!(!sync_rows().iter().any(|r| r.subject_key.ends_with(&pext(&est))), "bans win");
    assert!(skin::with_skin_conn(|c| clear_banned(c, SPACE, &pext(&est))).unwrap());
    plan();
    assert!(sync_rows().iter().any(|r| r.subject_key.ends_with(&pext(&est))), "sent again after the unban");
}

#[test]
fn a_deprovisioned_person_is_held_and_never_reactivated_by_a_sync() {
    let _env = setup();
    let (est, _off) = ready();
    plan();
    let r = sync_rows().into_iter().find(|r| r.subject_key.ends_with(&pext(&est))).unwrap();
    assert_eq!(verdict_ok(&serde_json::from_str(r#"{"skipped":"deprovisioned"}"#).unwrap()), Verdict::Held(WHY_DEPROVISIONED.into()));
    hook::record_held(&r.id, 200, r#"{"skipped":"deprovisioned"}"#, WHY_DEPROVISIONED).unwrap();
    sql("UPDATE grants SET role_id = (SELECT id FROM roles WHERE name = 'office') WHERE kind = 'app' AND subject_id = ?1", &[&est.id]);
    plan();
    assert!(!sync_rows().iter().any(|r| r.subject_key.ends_with(&pext(&est))), "a list change doesn't reactivate");
    assert!(
        !rows().iter().any(|r| r.op == "provision" && r.subject_key == pext(&est) && r.state == outbox::STATE_PENDING),
        "no provision from the sync"
    );
}

#[test]
fn the_owner_is_skipped_and_a_queued_job_is_read_back_next_sweep() {
    let _env = setup();
    let (est, off) = ready();
    plan();
    for r in sync_rows() {
        let body = if r.subject_key.ends_with(&pext(&est)) { r#"{"skipped":"owner"}"# } else { r#"{"queued":true}"# };
        hook::record_done(&r.id, 200, body).unwrap();
    }
    assert_eq!(ledger(&pext(&est)).0, SYNCED, "skipped: owner is not an error");
    assert_eq!(ledger(&pext(&off)).0, QUEUED);
    plan();
    let again: Vec<_> = sync_rows();
    assert_eq!(again.len(), 1, "only the queued item is read back");
    assert!(again[0].subject_key.ends_with(&pext(&off)));
    hook::record_done(&again[0].id, 200, r#"{"unchanged":[],"level":"moderator"}"#).unwrap();
    assert_eq!(ledger(&pext(&off)).0, SYNCED);
}

#[test]
fn a_person_with_many_channels_gets_one_full_list() {
    let _env = setup();
    let (est, _off) = ready();
    // The App made 1,500 private channels at run time and gives estella all.
    let slugs: Vec<String> = (0..1500).map(|i| format!("job-{i}")).collect();
    skin::with_skin_conn(|c| {
        for s in &slugs {
            c.execute(
                "INSERT INTO tool_app_space_channels (space_id, slug, app_id, source, channel_id, name, sew_name, kind, private, user_limit, state, why, ensure_tries, updated_at)
                 VALUES (?1, ?2, ?3, 'app', ?4, ?2, ?2, 'text', 1, NULL, 'ready', NULL, 1, 1)",
                rusqlite::params![SPACE, s, APP, format!("chan_{s}")],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    })
    .unwrap();
    hook::set_app_member_lists(APP, &[AppList { principal_id: est.id.clone(), space_id: SPACE.into(), slugs: slugs.clone(), level: None }], "app:test").unwrap();
    plan();
    let b = sync_body(&pext(&est));
    assert_eq!(b.channel_ids.len(), 1501, "1,500 App channels + plans-desk, one item");
    assert!(b.channel_ids.len() <= sew_wire::MEMBER_CHANNELS_MAX);
    assert_eq!(verdict_ok(&serde_json::from_str(r#"{"queued":true}"#).unwrap()), Verdict::Queued, "over 1,000 changes: Sew runs a job");
}

#[test]
fn unknown_channels_are_reported_held_and_ensured_again_once() {
    let _env = setup();
    let (est, _off) = ready();
    plan();
    let r = sync_rows().into_iter().find(|r| r.subject_key.ends_with(&pext(&est))).unwrap();
    let body = r#"{"unknown_channel_ids":["chan_plans-desk"]}"#;
    let v = verdict_ok(&serde_json::from_str(body).unwrap());
    let Verdict::Held(why) = v else { panic!("unknown channels must hold: {v:?}") };
    hook::record_held(&r.id, 200, body, &why).unwrap();
    assert_eq!(ledger(&pext(&est)).0, HELD);
    plan();
    let ensures: Vec<_> = open_rows("channels_bulk");
    assert_eq!(ensures.len(), 1, "plans-desk is ensured again");
    let req: ChannelsBulkRequest = serde_json::from_str(&ensures[0].payload_raw).unwrap();
    assert_eq!(req.ops.iter().map(|o| (o.op.as_str(), o.slug.as_deref())).collect::<Vec<_>>(), vec![("ensure", Some("plans-desk"))]);
}

#[test]
fn member_not_provisioned_provisions_again_then_syncs_again() {
    let _env = setup();
    let (est, _off) = ready();
    plan();
    let r = sync_rows().into_iter().find(|r| r.subject_key.ends_with(&pext(&est))).unwrap();
    assert!(matches!(verdict_err(422, "invalid_state", "member_not_provisioned", "x"), Verdict::Reprovision(_)));
    hook::record_reprovision(&r.id, 422, "member_not_provisioned").unwrap();
    let all = rows();
    let prov = all.iter().rfind(|x| x.op == "provision" && x.subject_key == pext(&est) && x.state == outbox::STATE_PENDING).expect("a new provision");
    let again = all.iter().rfind(|x| x.op == "space_member_channels" && x.subject_key.ends_with(&pext(&est)) && x.state == outbox::STATE_PENDING).expect("the sync again");
    assert!(prov.seq < again.seq, "provision first, then the sync");
    assert_eq!(all.iter().find(|x| x.id == r.id).unwrap().state, outbox::STATE_SUPERSEDED);
}

#[test]
fn refusals_that_mean_a_k2_bug_hold_and_not_app_governed_retries() {
    for r in ["box_user_not_allowed", "level_above_ceiling", "agent_level"] {
        assert!(matches!(verdict_err(422, "invalid_state", r, "x"), Verdict::Held(_)), "{r}");
    }
    assert!(matches!(verdict_err(409, "conflict", "not_app_governed", "x"), Verdict::Retry(_)));
    assert!(matches!(verdict_err(400, "validation_failed", "", "x"), Verdict::Blocked(_)));
}

#[test]
fn nothing_is_planned_or_sent_without_sews_flag() {
    let _env = setup();
    let (_est, _off) = ready();
    hook::note_features_for_tests(&[]).unwrap();
    let s = plan();
    assert_eq!(s, Summary::default(), "the flag gates the sync");
    let body = serde_json::Map::from_iter([("governance".to_string(), json!("app"))]);
    assert_eq!(sendable(&body, &[]), None);
    assert_eq!(sendable(&body, &[features::APP_GOVERNED_SPACES.to_string()]), Some(json!({"governance": "app"})));
    assert_eq!(hook::set_space_governance(APP, SPACE, "sew", "owner-token"), Err(hook::GovernanceRefusal::FeatureMissing));
}

#[test]
fn governance_needs_a_binding_and_a_key_and_an_unbind_forgets_it() {
    let _env = setup();
    assert_eq!(hook::set_space_governance(APP, SPACE, "app", "owner-token"), Err(hook::GovernanceRefusal::SpaceUnbound));
    skin::with_skin_conn(|c| app_space::bind(c, APP, SPACE, "owner-token", 1).map(|_| ())).unwrap();
    assert_eq!(hook::set_space_governance(OTHER_APP, SPACE, "app", "owner-token"), Err(hook::GovernanceRefusal::AppMismatch(APP.into())));
    assert_eq!(hook::set_space_governance(APP, SPACE, "app", "owner-token"), Err(hook::GovernanceRefusal::ChatKeyRequired));
    key(APP, "moderator", 500);
    assert_eq!(hook::set_space_governance(APP, SPACE, "app", "owner-token"), Ok(true));
    assert_eq!(hook::space_governance(SPACE).unwrap(), (Some("app".into()), None));
    // Binding never switches; the switch is planned on the settings sweep.
    let ops = skin::with_skin_conn(|c| hook::home::plan(c, &Default::default())).unwrap();
    let sw = ops.iter().find(|o| o.op == Op::SpaceSettings).expect("the switch is planned");
    assert_eq!(sw.payload, json!({"governance": "app"}));
    assert_eq!(hook::home::path_for(Op::SpaceSettings, &sw.subject_key).as_deref(), Some("/mgmt/v1/spaces/spc_meeting1/settings"));
    skin::with_skin_conn(|c| app_space::unbind(c, APP, SPACE, false, "owner-token", 2).map(|_| ())).unwrap();
    assert_eq!(hook::space_governance(SPACE).unwrap(), (None, None), "Sew resets to sew on unbind; K2 forgets its wish");
}

#[test]
fn box_users_present_queues_removes_of_box_users_only_and_holds_the_switch_until_they_are_out() {
    let _env = setup();
    key(APP, "moderator", 500);
    skin::with_skin_conn(|c| app_space::bind(c, APP, SPACE, "owner-token", 1).map(|_| ())).unwrap();
    hook::set_space_governance(APP, SPACE, "app", "owner-token").unwrap();
    let now = outbox::now_secs();
    let ops = skin::with_skin_conn(|c| hook::home::plan(c, &Default::default())).unwrap();
    let ids = skin::with_skin_conn(|c| outbox::enqueue(c, &ops, "owner-token", now)).unwrap();
    let sw = rows().into_iter().find(|r| ids.contains(&r.id) && r.op == "space_settings").expect("switch row");
    let connect = "k2:connect:3c4d5e6f-0000-4000-8000-000000000003";
    let principal = "k2:principal:4d5e6f70-0000-4000-8000-000000000004";
    let n = hook::record_box_users_present(&sw.id, 409, "{}", &["k2:owner".into(), connect.into(), principal.into()]).unwrap();
    assert_eq!(n, 2, "the owner and the Connect login; never an App user");
    let removes: Vec<_> = open_rows("space_member_remove");
    let mut who: Vec<String> = removes.iter().map(|r| r.subject_key.clone()).collect();
    who.sort();
    assert_eq!(who, vec![format!("space:{SPACE}:{connect}"), format!("space:{SPACE}:k2:owner")]);
    // The switch waits for them.
    let busy = skin::with_skin_conn(|c| outbox::busy_subjects(c, CHAT_KIND)).unwrap();
    let again = skin::with_skin_conn(|c| hook::home::plan(c, &busy)).unwrap();
    assert!(!again.iter().any(|o| o.op == Op::SpaceSettings), "not while they are leaving");
    for r in removes {
        hook::record_done(&r.id, 200, r#"{"removed":true}"#).unwrap();
    }
    let busy = skin::with_skin_conn(|c| outbox::busy_subjects(c, CHAT_KIND)).unwrap();
    let again = skin::with_skin_conn(|c| hook::home::plan(c, &busy)).unwrap();
    assert!(again.iter().any(|o| o.op == Op::SpaceSettings), "the switch goes again");
    let v = hook::governed_view(APP).unwrap();
    assert_eq!(v.spaces[0].leaving.iter().map(|l| l.state.as_str()).collect::<Vec<_>>(), vec!["gone", "gone"]);
}

#[test]
fn manifest_channels_ensure_rename_archive_and_a_type_change_fails() {
    let _env = setup();
    let (_e, _o) = ready();
    let mut block = standard_block();
    block.channels[1].name = "Plans desk (new)".into();
    block.channels.retain(|c| c.slug != "by-room");
    block.channels[2].kind = "voice".into(); // office-only: text → voice
    block.channels.push(ch("estimates", true, &["estimator"], &[], &[]));
    facts(block);
    plan();
    let r = open_rows("channels_bulk");
    assert_eq!(r.len(), 1);
    let req: ChannelsBulkRequest = serde_json::from_str(&r[0].payload_raw).unwrap();
    let mut got: Vec<(String, String)> = req.ops.iter().map(|o| (o.op.clone(), o.slug.clone().unwrap())).collect();
    got.sort();
    assert_eq!(
        got,
        vec![("archive".into(), "by-room".into()), ("ensure".into(), "estimates".into()), ("rename".into(), "plans-desk".into())]
    );
    let v = hook::governed_view(APP).unwrap();
    let oo = v.spaces[0].channels.iter().find(|c| c.slug == "office-only").unwrap();
    assert_eq!(oo.state, CH_FAILED, "a type change is never sent: {oo:?}");
}

#[test]
fn the_service_owner_is_deprovisioned_after_the_apps_unbinds() {
    let _env = setup();
    key(APP, "moderator", 500);
    skin::with_skin_conn(|c| app_space::bind(c, APP, SPACE, "owner-token", 1).map(|_| ())).unwrap();
    hook::note_features_for_tests(&[features::APP_GOVERNED_SPACES]).unwrap();
    hook::app_spaces_app_removed(APP, "owner-token").unwrap();
    let busy = skin::with_skin_conn(|c| outbox::busy_subjects(c, CHAT_KIND)).unwrap();
    let ops = skin::with_skin_conn(|c| plan_service_retire(c, &busy, 5)).unwrap();
    assert!(ops.is_empty(), "the unbind (Sew lacks space_unbind here) is still unsent: {ops:?}");
    sql("UPDATE tool_app_space_unbinds SET done_at = 6 WHERE app_id = ?1", &[&APP]);
    let ops = skin::with_skin_conn(|c| plan_service_retire(c, &busy, 7)).unwrap();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].0.op, Op::Deprovision);
    assert_eq!(ops[0].0.payload, json!({"external_id": format!("app:{APP}")}));
    assert!(skin::with_skin_conn(|c| plan_service_retire(c, &busy, 8)).unwrap().is_empty(), "once");
}

#[test]
fn keyed_app_users_get_a_member_grant_and_lose_it_with_their_login() {
    let _env = setup();
    key(APP, "moderator", 2);
    let mut ps = Vec::new();
    for n in ["ann", "bob", "cy"] {
        let p = skin::add_principal(n).unwrap();
        grant(SkinGrantWrite {
            subject_kind: "principal".into(),
            subject_id: p.id.clone(),
            kind: "app".into(),
            target_id: APP.into(),
            role: None,
            scope: Some("[]".into()),
            created_by: Some("owner-token".into()),
        });
        ps.push(p);
    }
    let (add, drop) = skin::with_skin_conn(plan_auto_grants).unwrap();
    assert_eq!(add.len(), 2, "max_members = 2: {add:?}");
    assert!(drop.is_empty());
    assert_eq!(hook::governed_auto_grants().unwrap(), (2, 0));
    let made: i64 = skin::with_skin_conn(|c| {
        c.query_row("SELECT COUNT(*) FROM grants WHERE kind = 'chat' AND created_by = ?1", [format!("chat-key:{APP}")], |r| r.get(0))
            .map_err(|e| e.to_string())
    })
    .unwrap();
    assert_eq!(made, 2);
    // One of the two people K2 granted loses their login: their K2-made
    // chat grant goes with it (and only theirs).
    let (gid, who): (String, String) = skin::with_skin_conn(|c| {
        c.query_row("SELECT id, subject_id FROM grants WHERE kind = 'chat' AND created_by = ?1 ORDER BY subject_id LIMIT 1", [format!("chat-key:{APP}")], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())
    })
    .unwrap();
    assert!(ps.iter().any(|p| p.id == who));
    sql("DELETE FROM grants WHERE kind = 'app' AND subject_id = ?1", &[&who]);
    let (_add, drop) = skin::with_skin_conn(plan_auto_grants).unwrap();
    assert_eq!(drop, vec![gid]);
}

#[test]
fn the_service_owner_external_id_round_trips_and_app_native_ids_stay_other() {
    let id = ExternalId::parse(&format!("app:{APP}")).unwrap();
    assert_eq!(id, ExternalId::AppService(APP.into()));
    assert_eq!(id.to_string(), format!("app:{APP}"));
    assert_eq!(serde_json::from_str::<ExternalId>(&format!("\"app:{APP}\"")).unwrap(), id);
    let native: ExternalId = serde_json::from_str(&format!("\"app:{APP}:user-1\"")).unwrap();
    assert!(!native.is_known(), "App-native ids are read leniently, never sent");
    assert!(ExternalId::parse("app:host").is_err());
    assert!(ExternalId::K2Owner.is_box_user());
    assert!(!id.is_box_user());
    assert_eq!(
        sew_wire::member_channels_path(SPACE, &ExternalId::parse("k2:principal:4d5e6f70-0000-4000-8000-000000000004").unwrap()),
        "/mgmt/v1/spaces/spc_meeting1/members/k2%3Aprincipal%3A4d5e6f70-0000-4000-8000-000000000004/channels"
    );
}
