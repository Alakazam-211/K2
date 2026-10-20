//! K31 unit tests: App Space membership comes only from App user grants
//! (rev 147), never from a Connect role, the owner or an agent.

use std::collections::HashSet;

use super::*;
use crate::skin::{self, SkinGrantWrite};
use crate::tool_hook::{self as hook, outbox, CHAT_KIND};

const SPACE: &str = "spc_hearth01";
const SPACE2: &str = "spc_booking1";

struct Env {
    _home: crate::test_env::TempHome,
}

fn setup() -> Env {
    let home = crate::test_env::TempHome::new();
    hook::allow_target_off_linux_for_tests(true);
    let _ = hook::take_reconcile_request();
    hook::set_target(CHAT_KIND, true, Some("/tmp/k2-test-sew.sock"), Some("/tmp/k2-test-sew.token"), "owner-token")
        .expect("enable chat target");
    Env { _home: home }
}

fn person(name: &str) -> skin::SkinPrincipal {
    skin::add_principal(name).expect("add principal")
}

fn grant(subject_kind: &str, subject: &str, kind: &str, target: &str, scope: Option<&str>) -> skin::SkinGrant {
    skin::create_grant(&SkinGrantWrite {
        subject_kind: subject_kind.into(),
        subject_id: subject.into(),
        kind: kind.into(),
        target_id: target.into(),
        role: None,
        scope: scope.map(str::to_string),
        created_by: Some("owner-token".into()),
    })
    .unwrap_or_else(|e| panic!("grant {subject_kind}:{subject} {kind}:{target}: {e}"))
}

fn app_user(p: &skin::SkinPrincipal, app: &str) -> skin::SkinGrant {
    grant("principal", &p.id, "app", app, Some("[]"))
}

fn chat(p: &skin::SkinPrincipal, scope: &str) -> skin::SkinGrant {
    grant("principal", &p.id, "chat", "", Some(scope))
}

fn pext(id: &str) -> String {
    format!("k2:principal:{id}")
}

fn rows() -> Vec<outbox::OutboxRow> {
    let mut r = hook::log(1000, None).expect("outbox log");
    r.reverse();
    r
}

fn space_rows() -> Vec<outbox::OutboxRow> {
    rows().into_iter().filter(|r| r.subject_key.starts_with(SUBJECT_PREFIX)).collect()
}

fn open_space_rows() -> Vec<outbox::OutboxRow> {
    space_rows().into_iter().filter(|r| r.state == outbox::STATE_PENDING).collect()
}

fn desired_pairs() -> HashSet<(String, String)> {
    skin::with_skin_conn(build_desired)
        .expect("desired")
        .into_iter()
        .map(|w| (w.space_id, w.ext.to_string()))
        .collect()
}

fn ledger_state(space: &str, ext: &str) -> Option<String> {
    skin::with_skin_conn(|c| {
        c.query_row(
            "SELECT state FROM tool_app_space_members WHERE space_id = ?1 AND external_id = ?2",
            rusqlite::params![space, ext],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    })
    .expect("ledger read")
}

fn added_body(ext: &str) -> String {
    format!(r#"{{"added":[{{"external_id":"{ext}","member_id":"mem_1"}}],"already":[],"skipped":[],"not_added":[]}}"#)
}

fn bind(app: &str, space: &str) -> Summary {
    let (created, s) = hook::app_space_bind(app, space, "owner-token").expect("bind");
    assert!(created, "a new binding");
    s
}

#[test]
fn desired_is_app_grant_plus_chat_member_principals_only() {
    let _e = setup();
    // Connect Owner and Admin, both with their own `connect` chat grants
    // (Sew accounts): server control, never App Space members by role.
    crate::connect_users::add_user("boss", "pw-boss-long").unwrap();
    crate::connect_users::set_role("boss", crate::connect_users::Role::Owner).unwrap();
    crate::connect_users::add_user("adm", "pw-adm-long").unwrap();
    crate::connect_users::set_role("adm", crate::connect_users::Role::Admin).unwrap();
    for login in ["boss", "adm"] {
        let id = crate::connect_users::user_uuid(login).unwrap().unwrap();
        grant("connect", &id, "chat", "", Some("member"));
    }
    // A Connect login can't hold an App grant at all.
    let boss_id = crate::connect_users::user_uuid("boss").unwrap().unwrap();
    let e = skin::create_grant(&SkinGrantWrite {
        subject_kind: "connect".into(),
        subject_id: boss_id,
        kind: "app".into(),
        target_id: "hearth".into(),
        role: None,
        scope: Some("[]".into()),
        created_by: Some("owner-token".into()),
    })
    .unwrap_err();
    assert_eq!(e, "subjectKind connect is only for kind=chat");

    let ann = person("ann"); // App user + chat member → in
    let bea = person("bea"); // App user + chat GUEST → out (Sew: guests via channels)
    let cy = person("cy"); // chat member, App grant for another App → only that App's Space
    let dee = person("dee"); // App user, chat member, App grant disabled → out
    let eve = person("eve"); // App user, no chat grant (no Sew account) → out
    app_user(&ann, "hearth");
    chat(&ann, "member");
    app_user(&bea, "hearth");
    chat(&bea, "guest");
    app_user(&cy, "booking");
    chat(&cy, "member");
    let dg = app_user(&dee, "hearth");
    chat(&dee, "member");
    skin::set_grant_enabled(&dg.id, false).unwrap();
    app_user(&eve, "hearth");
    // An agent with a chat grant is not an App user.
    let ws = uuid::Uuid::new_v4().to_string();
    {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, 'Ops', ?2, ?3)",
            rusqlite::params![ws, format!("/tmp/k2-k31-{ws}"), format!("ops{}", &ws[..8])],
        )
        .unwrap();
    }
    grant("workspace", &ws, "chat", "", Some("coworker"));

    assert!(desired_pairs().is_empty(), "nothing is bound yet");
    bind("hearth", SPACE);
    bind("booking", SPACE2);
    assert_eq!(
        desired_pairs(),
        HashSet::from([(SPACE.to_string(), pext(&ann.id)), (SPACE2.to_string(), pext(&cy.id))])
    );
    // The ops K31 queued name only those two; never k2:connect, k2:owner
    // or k2:workspace.
    let subjects: HashSet<String> = space_rows().into_iter().map(|r| r.subject_key).collect();
    assert_eq!(
        subjects,
        HashSet::from([
            format!("space:{SPACE}:{}", pext(&ann.id)),
            format!("space:{SPACE2}:{}", pext(&cy.id)),
        ])
    );
    for r in space_rows() {
        let p: serde_json::Value = serde_json::from_str(&r.payload_raw).unwrap();
        for ext in p["external_ids"].as_array().unwrap() {
            let ext = ext.as_str().unwrap();
            assert!(ext.starts_with("k2:principal:"), "only App users: {ext}");
        }
    }
}

#[test]
fn add_waits_for_the_provision_then_revoke_removes() {
    let _e = setup();
    let ann = person("ann");
    let app = app_user(&ann, "hearth");
    let c = chat(&ann, "member");
    let s = bind("hearth", SPACE);
    assert_eq!(s.adds, 1, "{s:?}");
    let add = open_space_rows().remove(0);
    assert_eq!(add.op, "space_member_add");
    assert_eq!(add.tool, CHAT_KIND);
    assert_eq!(add.after_subject.as_deref(), Some(pext(&ann.id).as_str()), "after the person's provision");
    assert_eq!(add.grant_id.as_deref(), Some(app.id.as_str()));
    assert_eq!(add.grant_rev.as_deref(), Some(format!("{}:[]:1|{}:member:1", app.id, c.id).as_str()));
    let p: serde_json::Value = serde_json::from_str(&add.payload_raw).unwrap();
    assert_eq!(p, serde_json::json!({"space_id": SPACE, "app_id": "hearth", "external_ids": [pext(&ann.id)]}));
    // The provision row is ahead of it, so the add is not due yet.
    let due: Vec<String> = hook::due(CHAT_KIND, 100).unwrap().into_iter().map(|r| r.subject_key).collect();
    assert!(due.contains(&pext(&ann.id)));
    assert!(!due.iter().any(|k| k.starts_with(SUBJECT_PREFIX)), "{due:?}");
    // Provision delivered → the add is due; Sew adds her.
    let prov = rows().into_iter().find(|r| r.subject_key == pext(&ann.id)).unwrap();
    hook::record_done(&prov.id, 200, "{}").unwrap();
    assert!(hook::due(CHAT_KIND, 100).unwrap().iter().any(|r| r.id == add.id));
    assert_eq!(ledger_state(SPACE, &pext(&ann.id)).as_deref(), Some(PENDING_ADD));
    hook::record_done(&add.id, 200, &added_body(&pext(&ann.id))).unwrap();
    assert_eq!(ledger_state(SPACE, &pext(&ann.id)).as_deref(), Some(ADDED));
    // Stable: a second plan queues nothing.
    let s = skin::with_skin_conn(|c| reconcile(c, "system:reconcile", outbox::now_secs())).unwrap();
    assert_eq!((s.adds, s.removes), (0, 0), "{s:?}");
    // The App grant goes → she leaves the Space.
    skin::delete_grant_by_id(&app.id).unwrap();
    let s = skin::with_skin_conn(|c| reconcile(c, "system:reconcile", outbox::now_secs())).unwrap();
    assert_eq!(s.removes, 1, "{s:?}");
    let rm = open_space_rows().into_iter().find(|r| r.op == "space_member_remove").expect("remove row");
    let p: serde_json::Value = serde_json::from_str(&rm.payload_raw).unwrap();
    assert_eq!(p, serde_json::json!({"space_id": SPACE, "app_id": "hearth", "external_id": pext(&ann.id)}));
    assert_eq!(ledger_state(SPACE, &pext(&ann.id)).as_deref(), Some(PENDING_REMOVE));
    hook::record_done(&rm.id, 200, r#"{"removed":true}"#).unwrap();
    assert_eq!(ledger_state(SPACE, &pext(&ann.id)), None);
}

#[test]
fn held_stays_out_until_the_grant_changes() {
    let _e = setup();
    let ann = person("ann");
    app_user(&ann, "hearth");
    let c = chat(&ann, "member");
    bind("hearth", SPACE);
    let add = open_space_rows().remove(0);
    hook::record_held(&add.id, 200, r#"{"skipped":["mem_1"]}"#, "skipped: banned in this Space").unwrap();
    assert_eq!(ledger_state(SPACE, &pext(&ann.id)).as_deref(), Some(HELD));
    let s = skin::with_skin_conn(|c| reconcile(c, "system:reconcile", outbox::now_secs())).unwrap();
    assert_eq!((s.adds, s.held_kept), (0, 1), "held is not re-added: {s:?}");
    // Her chat grant changes (member → guest → member: a new rev) → K2
    // asks again.
    skin::update_grant_scope(&c.id, "guest", "owner-token").unwrap();
    let s = skin::with_skin_conn(|c| reconcile(c, "system:reconcile", outbox::now_secs())).unwrap();
    assert_eq!((s.adds, s.held_forgotten), (0, 1), "a guest is not wanted; the hold is forgotten: {s:?}");
    skin::update_grant_scope(&c.id, "member", "owner-token").unwrap();
    let s = skin::with_skin_conn(|c| reconcile(c, "system:reconcile", outbox::now_secs())).unwrap();
    assert_eq!(s.adds, 1, "{s:?}");
}

#[test]
fn verdicts_from_seq_answers() {
    let ext = pext("0b6f2c1e-9d3a-4c6e-8b1f-2a3b4c5d6e7f");
    let payload = serde_json::json!({"space_id": SPACE, "app_id": "hearth", "external_ids": [ext]}).to_string();
    assert_eq!(add_verdict(&payload, &added_body(&ext)), AddVerdict::Added);
    let already = format!(r#"{{"added":[],"already":[{{"external_id":"{ext}","member_id":"m"}}],"skipped":[],"not_added":[]}}"#);
    assert_eq!(add_verdict(&payload, &already), AddVerdict::Added);
    let np = format!(r#"{{"added":[],"already":[],"skipped":[],"not_added":[{{"external_id":"{ext}","reason":"not_provisioned"}}]}}"#);
    assert_eq!(add_verdict(&payload, &np), AddVerdict::NotProvisioned);
    let guest = format!(r#"{{"added":[],"already":[],"skipped":[],"not_added":[{{"external_id":"{ext}","reason":"guest"}}]}}"#);
    assert_eq!(add_verdict(&payload, &guest), AddVerdict::Held("not added: guest".into()));
    let banned = r#"{"added":[],"already":[],"skipped":["mem_9"],"not_added":[]}"#;
    assert_eq!(add_verdict(&payload, banned), AddVerdict::Held("skipped: banned in this Space".into()));
    assert!(matches!(add_verdict(&payload, r#"{"added":[],"already":[]}"#), AddVerdict::Unknown(_)));
    assert!(matches!(add_verdict(&payload, "not json"), AddVerdict::Unknown(_)));
    assert!(matches!(add_verdict("{}", &added_body(&ext)), AddVerdict::Unknown(_)));
}

#[test]
fn bind_rules_and_unbind_removes_what_k2_added() {
    let _e = setup();
    for (app, space, want) in [
        ("host", SPACE, "the box login"),
        ("Hearth", SPACE, "not an App id"),
        ("hearth", "spc bad", "not a Sew Space id"),
        ("", SPACE, "not an App id"),
    ] {
        let e = hook::app_space_bind(app, space, "owner-token").unwrap_err();
        assert!(e.contains(want), "{app}/{space}: {e}");
    }
    bind("hearth", SPACE);
    let (created, _) = hook::app_space_bind("hearth", SPACE, "owner-token").unwrap();
    assert!(!created, "the same App again is a no-op");
    let e = hook::app_space_bind("booking", SPACE, "owner-token").unwrap_err();
    assert!(e.starts_with("space_bound:"), "{e}");
    let views = hook::app_spaces().unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].binding.app_id, "hearth");
    assert_eq!(serde_json::to_value(&views[0]).unwrap()["spaceId"], SPACE);

    let ann = person("ann");
    app_user(&ann, "hearth");
    chat(&ann, "member");
    let s = skin::with_skin_conn(|c| reconcile(c, "system:reconcile", outbox::now_secs())).unwrap();
    assert_eq!(s.adds, 1);
    let add = open_space_rows().remove(0);
    hook::record_done(&add.id, 200, &added_body(&pext(&ann.id))).unwrap();
    assert_eq!(hook::app_spaces().unwrap()[0].members, Counts { added: 1, held: 0, pending: 0 });
    let (gone, s) = hook::app_space_unbind("hearth", SPACE, "owner-token").unwrap();
    assert!(gone);
    assert_eq!(s.removes, 1, "{s:?}");
    assert!(hook::app_spaces().unwrap().is_empty());
    assert_eq!(ledger_state(SPACE, &pext(&ann.id)).as_deref(), Some(PENDING_REMOVE));
}

#[test]
fn a_chat_grant_change_asks_for_a_sweep_once_a_space_is_bound() {
    let _e = setup();
    bind("hearth", SPACE);
    let ann = person("ann");
    app_user(&ann, "hearth");
    let _ = hook::take_reconcile_request();
    let c = chat(&ann, "member");
    assert!(hook::take_reconcile_request(), "chat grant create → sweep (K31)");
    skin::update_grant_scope(&c.id, "guest", "owner-token").unwrap();
    assert!(hook::take_reconcile_request(), "chat grant update → sweep");
    skin::set_grant_enabled(&c.id, false).unwrap();
    assert!(hook::take_reconcile_request(), "chat grant disable → sweep");
    skin::delete_grant_by_id(&c.id).unwrap();
    assert!(hook::take_reconcile_request(), "chat grant delete → sweep");
}

#[test]
fn the_reconcile_sweep_plans_app_spaces_too() {
    let _e = setup();
    let ann = person("ann");
    app_user(&ann, "hearth");
    chat(&ann, "member");
    // A binding written without its own plan (e.g. by an older daemon).
    skin::with_skin_conn(|c| super::bind(c, "hearth", SPACE, "owner-token", 1).map(|_| ())).unwrap();
    assert!(space_rows().is_empty());
    hook::apply_reconcile(CHAT_KIND, &[]).unwrap();
    let r = space_rows();
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0].op, "space_member_add");
    assert_eq!(r[0].actor, hook::ACTOR_RECONCILE);
}
