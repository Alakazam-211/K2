//! tool_hook unit tests (S27 §20.2 k2-core list + K4 + HC2 + rev 147).
//!
//! Every test runs under a fresh temp HOME (`TempHome` holds the one env
//! lock, so these serialize with every other HOME-reading test) and a
//! fresh `skin.db`. Workspaces go into the in-memory test main DB with
//! unique ids/handles.

use std::collections::{HashMap, HashSet};

use super::chat::{self, ChatScope};
use super::outbox::{self, OwnerState, STATE_BLOCKED, STATE_DONE, STATE_PENDING};
use super::reconcile::{self, Desired};
use super::sew_wire::{
    AgentKind, ExternalId, InstallRole, MemberKind, MgmtPrincipal, ProfileInput, ProvisionRequest,
};
use super::*;
use crate::skin::{self, SkinGrantWrite};

struct Env {
    _home: crate::test_env::TempHome,
}

fn setup() -> Env {
    let home = crate::test_env::TempHome::new();
    allow_target_off_linux_for_tests(true);
    skin::HOOK_FAIL_BEFORE_COMMIT.store(false, Ordering::SeqCst);
    let _ = take_reconcile_request();
    Env { _home: home }
}

fn enable_chat() -> outbox::ToolTarget {
    set_target(
        CHAT_KIND,
        true,
        Some("/tmp/k2-test-sew.sock"),
        Some("/tmp/k2-test-sew.token"),
        "owner-token",
    )
    .expect("enable chat target")
}

fn add_ws(handle_prefix: &str) -> (String, String) {
    let id = uuid::Uuid::new_v4().to_string();
    let handle = format!("{handle_prefix}{}", &id[..8]);
    let db = crate::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            id,
            format!("Name {handle}"),
            format!("/tmp/k2-hook-ws-{id}"),
            handle
        ],
    )
    .expect("insert project");
    (id, handle)
}

fn drop_ws(id: &str) {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![id])
        .expect("delete project");
}

fn person(username: &str) -> skin::SkinPrincipal {
    skin::add_principal(username).expect("add principal")
}

fn grant(
    subject_kind: &str,
    subject: &str,
    scope: Option<&str>,
) -> Result<skin::SkinGrant, String> {
    skin::create_grant(&SkinGrantWrite {
        subject_kind: subject_kind.into(),
        subject_id: subject.into(),
        kind: "chat".into(),
        target_id: String::new(),
        role: None,
        scope: scope.map(str::to_string),
        created_by: Some("owner-token".into()),
    })
}

/// Oldest first.
fn rows() -> Vec<outbox::OutboxRow> {
    let mut r = log(500, None).expect("outbox log");
    r.reverse();
    r
}

fn rows_for(subject: &str) -> Vec<outbox::OutboxRow> {
    rows()
        .into_iter()
        .filter(|r| r.subject_key == subject)
        .collect()
}

fn row(id: &str) -> outbox::OutboxRow {
    skin::with_skin_conn(|c| outbox::get(c, id))
        .expect("outbox get")
        .expect("row exists")
}

fn payload(r: &outbox::OutboxRow) -> serde_json::Value {
    serde_json::from_str(&r.payload_raw).expect("payload json")
}

fn pext(id: &str) -> String {
    format!("k2:principal:{id}")
}

fn wext(id: &str) -> String {
    format!("k2:workspace:{id}")
}

/// Mark every open row done (as if Sew answered 2xx).
fn deliver_all() {
    for r in rows().into_iter().filter(|r| r.state == STATE_PENDING) {
        record_done(&r.id, 200, "{}").unwrap();
    }
}

// ── kind, target, scope grammar, validation ──────────────────────────

#[test]
fn chat_kind_accepted_and_target_defaults_to_sew() {
    let _e = setup();
    enable_chat();
    let ann = person("ann");
    let g = grant("principal", "ann", None).expect("chat grant by username");
    assert_eq!(g.kind, "chat");
    assert_eq!(g.target_id, "sew");
    assert_eq!(
        g.subject_id, ann.id,
        "username resolved to the principal id"
    );
    assert_eq!(
        g.scope.as_deref(),
        Some("member"),
        "principal scope defaults to member"
    );
    assert_eq!(g.role_id, None);
    let r = rows_for(&pext(&ann.id));
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0].op, "provision");
    assert_eq!(r[0].actor, "owner-token");
    assert_eq!(r[0].cause, "grant_create");
    assert_eq!(r[0].grant_id.as_deref(), Some(g.id.as_str()));
    let p = payload(&r[0]);
    assert_eq!(p["kind"], "person");
    assert_eq!(p["guest"], false);
    assert_eq!(p["profile"]["username"], "ann");
    assert_eq!(p["profile"]["display_name"], "ann");
    assert!(p["profile"].get("email").is_none());
}

#[test]
fn chat_scope_grammar_by_subject() {
    let _e = setup();
    enable_chat();
    let ann = person("ann");
    grant("principal", &ann.id, Some(" MEMBER ")).expect("member, normalized");
    skin::with_skin_conn(|c| {
        assert_eq!(
            chat::parse_input_scope(c, "principal", Some("guest"))?,
            ChatScope::Guest
        );
        assert_eq!(
            chat::parse_input_scope(c, "principal", None)?,
            ChatScope::Member
        );
        assert_eq!(
            chat::parse_input_scope(c, "workspace", Some("Coworker"))?,
            ChatScope::Coworker
        );
        assert_eq!(
            chat::parse_input_scope(c, "workspace", Some("assistant:ann"))?,
            ChatScope::AssistantOf(ann.id.clone()),
            "username stored as principal id"
        );
        assert_eq!(
            chat::parse_input_scope(c, "workspace", Some(&format!("assistant:{}", ann.id)))?,
            ChatScope::AssistantOf(ann.id.clone())
        );
        assert_eq!(
            chat::parse_input_scope(c, "workspace", Some("assistant:OWNER"))?,
            ChatScope::AssistantOwner
        );
        for (kind, bad) in [
            ("principal", "coworker"),
            ("principal", "assistant:owner"),
            ("principal", "admin"),
            ("workspace", "member"),
            ("workspace", "guest"),
            ("workspace", "assistant:"),
            ("workspace", "coworker:x"),
        ] {
            let e = chat::parse_input_scope(c, kind, Some(bad)).unwrap_err();
            assert_eq!(e, chat::SCOPE_ERR, "{kind} {bad}");
        }
        assert!(chat::parse_input_scope(c, "workspace", None)
            .unwrap_err()
            .contains("required for a workspace"));
        assert_eq!(
            chat::parse_input_scope(c, "workspace", Some("assistant:nobody")).unwrap_err(),
            "unknown assistant person 'nobody'"
        );
        Ok(())
    })
    .unwrap();
    for s in ["member", "guest", "coworker", "assistant:owner"] {
        assert_eq!(ChatScope::parse_stored(s).unwrap().as_stored(), s);
    }
    assert!(ChatScope::parse_stored("assistant:ann").is_err());
    assert!(ChatScope::parse_stored("Member").is_err());
}

#[test]
fn chat_rejects_role_other_target_non_uuid_and_long_username() {
    let _e = setup();
    enable_chat();
    let ann = person("ann");
    let mut w = SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: ann.id.clone(),
        kind: "chat".into(),
        target_id: "sew".into(),
        role: Some("r".into()),
        scope: None,
        created_by: None,
    };
    assert_eq!(
        skin::create_grant(&w).unwrap_err(),
        "roleId is only set for kind=app"
    );
    w.role = None;
    w.target_id = "slack".into();
    assert_eq!(
        skin::create_grant(&w).unwrap_err(),
        "chat target must be 'sew'"
    );
    // A principal row with a non-uuid id (old fixtures stored 'p1').
    skin::with_skin_conn(|c| {
        c.execute(
            "INSERT INTO principals (id, username, created_at) VALUES ('p1', 'legacy', 0)",
            [],
        )
        .map_err(|e| e.to_string())
    })
    .unwrap();
    assert_eq!(
        grant("principal", "legacy", None).unwrap_err(),
        "id 'p1' is not a UUID; chat needs one"
    );
    // A 65-char username: new ones are refused at creation (max 64, 0.46.4),
    // so this is an older row, stored directly like 'p1' above.
    let long = "a".repeat(65);
    let long_id = uuid::Uuid::new_v4().to_string();
    skin::with_skin_conn(|c| {
        c.execute(
            "INSERT INTO principals (id, username, created_at) VALUES (?1, ?2, 0)",
            rusqlite::params![long_id, long],
        )
        .map_err(|e| e.to_string())
    })
    .unwrap();
    assert_eq!(
        grant("principal", &long, None).unwrap_err(),
        format!("username '{long}' is longer than 64 characters; chat cannot use it")
    );
    // A one-character workspace handle is refused before enqueue (S27-A1).
    let (wid, _) = add_ws("x");
    {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute(
            "UPDATE projects SET handle = 'q' WHERE id = ?1",
            rusqlite::params![wid],
        )
        .unwrap();
    }
    let e = grant("workspace", &wid, Some("coworker")).unwrap_err();
    assert!(e.contains("must be 2–64 characters"), "{e}");
    assert_eq!(
        grant("principal", "ghost", None).unwrap_err(),
        "unknown principal id 'ghost'"
    );
    assert_eq!(
        grant("workspace", "no-such-ws-zz", Some("coworker")).unwrap_err(),
        "unknown workspace 'no-such-ws-zz'"
    );
    assert!(
        rows().iter().all(|r| r.subject_key == "k2:owner"),
        "nothing but the owner enqueued"
    );
    drop_ws(&wid);
}

#[test]
fn chat_rejected_when_target_disabled_and_off_linux_enable_refused() {
    let _e = setup();
    person("ann");
    assert_eq!(
        grant("principal", "ann", None).unwrap_err(),
        CHAT_NOT_ENABLED
    );
    enable_chat();
    set_target(CHAT_KIND, false, None, None, "owner-token").unwrap();
    assert_eq!(
        grant("principal", "ann", None).unwrap_err(),
        CHAT_NOT_ENABLED
    );
    allow_target_off_linux_for_tests(false);
    let r = set_target(CHAT_KIND, true, None, None, "owner-token");
    if cfg!(target_os = "linux") {
        assert!(r.is_ok(), "Linux may enable: {r:?}");
    } else {
        assert_eq!(r.unwrap_err(), "Sew runs on Linux only");
    }
    assert!(
        !set_target(CHAT_KIND, false, None, None, "owner-token")
            .unwrap()
            .enabled
    );
    allow_target_off_linux_for_tests(true);
}

#[test]
fn assistant_rules_member_person_guest_refused_owner_ok() {
    let _e = setup();
    enable_chat();
    let ann = person("ann");
    person("gus");
    let (w1, _) = add_ws("asst");
    assert_eq!(
        grant("workspace", &w1, Some("assistant:ann")).unwrap_err(),
        "assistant person 'ann' needs a member chat grant first"
    );
    grant("principal", "gus", Some("guest")).unwrap();
    assert_eq!(
        grant("workspace", &w1, Some("assistant:gus")).unwrap_err(),
        "assistant person 'gus' is a chat guest; guests cannot have assistants"
    );
    grant("principal", "ann", None).unwrap();
    let g = grant("workspace", &w1, Some("assistant:ann")).expect("assistant of a member");
    assert_eq!(g.scope, Some(format!("assistant:{}", ann.id)));
    let r = rows_for(&wext(&w1));
    assert_eq!(r.len(), 1);
    let p = payload(&r[0]);
    assert_eq!(p["kind"], "agent");
    assert_eq!(p["agent_kind"], "assistant");
    assert_eq!(p["assistant_of"], pext(&ann.id));
    assert_eq!(
        r[0].after_subject.as_deref(),
        Some(pext(&ann.id).as_str()),
        "waits for the person"
    );
    let (w2, _) = add_ws("own");
    let g2 = grant("workspace", &w2, Some("assistant:owner")).expect("assistant of the owner");
    assert_eq!(g2.scope.as_deref(), Some("assistant:owner"));
    assert_eq!(
        payload(&rows_for(&wext(&w2))[0])["assistant_of"],
        "k2:owner"
    );
    drop_ws(&w1);
    drop_ws(&w2);
}

#[test]
fn workspace_subject_by_handle_provisions_a_coworker_agent() {
    let _e = setup();
    enable_chat();
    let (wid, handle) = add_ws("sales");
    let g = grant("workspace", &handle, Some("coworker")).expect("by handle");
    assert_eq!(g.subject_id, wid, "handle resolved to projects.id");
    let r = rows_for(&wext(&wid));
    let p = payload(&r[0]);
    assert_eq!(p["agent_kind"], "coworker");
    assert!(p.get("assistant_of").is_none());
    assert_eq!(p["profile"]["username"], handle);
    assert_eq!(p["profile"]["display_name"], format!("Name {handle}"));
    drop_ws(&wid);
}

// ── outbox durability and shape ──────────────────────────────────────

#[test]
fn outbox_row_is_in_the_grant_transaction() {
    let _e = setup();
    enable_chat();
    person("ann");
    let before = rows().len();
    skin::HOOK_FAIL_BEFORE_COMMIT.store(true, Ordering::SeqCst);
    let r = grant("principal", "ann", None);
    skin::HOOK_FAIL_BEFORE_COMMIT.store(false, Ordering::SeqCst);
    assert_eq!(r.unwrap_err(), "injected failure before commit");
    assert!(
        skin::list_grants()
            .unwrap()
            .iter()
            .all(|g| g.kind != "chat"),
        "rolled-back grant must not exist"
    );
    assert_eq!(
        rows().len(),
        before,
        "rolled-back write leaves no outbox row"
    );
    // A committed write always has its row on disk (a crash between the
    // commit and the send loses nothing: the next worker finds it).
    let g = grant("principal", "ann", None).unwrap();
    let path = crate::paths::k2_home().join("skin.db");
    let raw = rusqlite::Connection::open(&path).unwrap();
    let n: i64 = raw
        .query_row(
            "SELECT COUNT(*) FROM tool_outbox WHERE grant_id = ?1 AND state = 'pending'",
            rusqlite::params![g.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1, "the committed row is on disk, pending");
}

#[test]
fn idem_key_format_and_stable_across_retries() {
    let _e = setup();
    enable_chat();
    person("ann");
    grant("principal", "ann", None).unwrap();
    for r in rows() {
        assert!(r.idem_key.starts_with("k2ob_"), "{}", r.idem_key);
        assert_eq!(r.idem_key.len(), 37);
        assert!(sew_wire::is_idempotency_key(&r.idem_key));
        assert_eq!(
            r.idem_key,
            outbox::idem_key_for(&r.id),
            "derived from the row id"
        );
    }
    let r = rows().into_iter().last().unwrap();
    record_retry(&r.id, Some(503), "unavailable: x", 5).unwrap();
    let again = row(&r.id);
    assert_eq!(again.idem_key, r.idem_key, "a retry keeps the key");
    assert_eq!(
        again.payload_raw, r.payload_raw,
        "a retry resends the same bytes"
    );
    assert_eq!((again.state.as_str(), again.attempts), (STATE_PENDING, 1));
    assert!(again.next_attempt_at >= r.created_at + 5);
}

#[test]
fn update_supersedes_only_unattempted_updates() {
    let _e = setup();
    enable_chat();
    let ann = person("ann");
    grant("principal", "ann", None).unwrap();
    deliver_all();
    skin::set_principal_full_name("ann", Some("Ann One")).unwrap();
    let first = rows_for(&pext(&ann.id)).into_iter().last().unwrap();
    assert_eq!(first.op, "update");
    skin::set_principal_full_name("ann", Some("Ann Two")).unwrap();
    assert_eq!(
        row(&first.id).state,
        "superseded",
        "unattempted update replaced"
    );
    let second = rows_for(&pext(&ann.id)).into_iter().last().unwrap();
    record_retry(&second.id, None, "connect refused", 1).unwrap();
    skin::set_principal_full_name("ann", Some("Ann Three")).unwrap();
    assert_eq!(
        row(&second.id).state,
        STATE_PENDING,
        "an attempted update is never superseded"
    );
    // A provision is never superseded either.
    let (wid, _) = add_ws("sup");
    let g = grant("workspace", &wid, Some("coworker")).unwrap();
    let prov = rows_for(&wext(&wid)).into_iter().last().unwrap();
    skin::delete_grant_by_id(&g.id).unwrap();
    assert_eq!(
        row(&prov.id).state,
        STATE_PENDING,
        "provision then deprovision both run"
    );
    drop_ws(&wid);
}

#[test]
fn scope_updates_member_guest_and_kind_flip_refused() {
    let _e = setup();
    enable_chat();
    let ann = person("ann");
    let g = grant("principal", "ann", None).unwrap();
    deliver_all();
    let up = skin::update_grant_scope(&g.id, "guest", "user:boss").expect("member → guest");
    assert_eq!(up.scope.as_deref(), Some("guest"));
    let last = rows_for(&pext(&ann.id)).into_iter().last().unwrap();
    assert_eq!(
        (last.op.as_str(), last.cause.as_str(), last.actor.as_str()),
        ("provision", "grant_update", "user:boss")
    );
    assert_eq!(payload(&last)["guest"], true);
    // Same scope again: no new row.
    let n = rows().len();
    skin::update_grant_scope(&g.id, "guest", "owner-token").unwrap();
    assert_eq!(rows().len(), n);
    // Agent kind flips in place are refused.
    let (wid, _) = add_ws("flip");
    let wg = grant("workspace", &wid, Some("coworker")).unwrap();
    assert_eq!(
        skin::update_grant_scope(&wg.id, "assistant:owner", "owner-token").unwrap_err(),
        chat::KIND_FLIP_ERR
    );
    assert_eq!(
        skin::update_grant_scope(&wg.id, "member", "owner-token").unwrap_err(),
        chat::SCOPE_ERR
    );
    // Non-hooked kinds cannot be updated.
    let app = skin::create_grant(&SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: ann.id.clone(),
        kind: "mailbox".into(),
        target_id: "box1".into(),
        role: None,
        scope: Some("read".into()),
        created_by: None,
    })
    .unwrap();
    assert_eq!(
        skin::update_grant_scope(&app.id, "write", "owner-token").unwrap_err(),
        "only chat grants can be updated"
    );
    drop_ws(&wid);
}

#[test]
fn person_grant_delete_cascades_assistant_grants_and_guest_flip_refused() {
    let _e = setup();
    enable_chat();
    let ann = person("ann");
    let g = grant("principal", "ann", None).unwrap();
    let (w1, _) = add_ws("ca");
    let (w2, _) = add_ws("cb");
    grant("workspace", &w1, Some("assistant:ann")).unwrap();
    grant("workspace", &w2, Some("coworker")).unwrap();
    assert_eq!(
        skin::update_grant_scope(&g.id, "guest", "owner-token").unwrap_err(),
        "remove the assistant grants for 'ann' first"
    );
    assert_eq!(
        skin::set_grant_enabled(&g.id, false).unwrap_err(),
        "remove the assistant grants for 'ann' first"
    );
    let cascaded = skin::delete_grant_by_id_with_cascade(&g.id, "owner-token")
        .unwrap()
        .expect("deleted");
    assert_eq!(cascaded.len(), 1);
    assert_eq!(cascaded[0].subject_id, w1);
    let left: Vec<_> = skin::list_grants()
        .unwrap()
        .into_iter()
        .filter(|g| g.kind == "chat")
        .collect();
    assert_eq!(left.len(), 1, "only the coworker remains: {left:?}");
    assert_eq!(left[0].subject_id, w2);
    let a = rows_for(&wext(&w1)).into_iter().last().unwrap();
    let p = rows_for(&pext(&ann.id)).into_iter().last().unwrap();
    assert_eq!(
        (a.op.as_str(), p.op.as_str()),
        ("deprovision", "deprovision")
    );
    assert!(
        a.seq < p.seq,
        "assistant deprovision is queued before the person's"
    );
    drop_ws(&w1);
    drop_ws(&w2);
}

#[test]
fn principal_delete_deletes_chat_grants_and_enqueues_deprovision() {
    let _e = setup();
    enable_chat();
    let ann = person("ann");
    grant("principal", "ann", None).unwrap();
    let (wid, _) = add_ws("pd");
    grant("workspace", &wid, Some("assistant:ann")).unwrap();
    assert!(skin::remove_principal_as("ann", "user:boss").unwrap());
    assert!(skin::list_grants()
        .unwrap()
        .iter()
        .all(|g| g.kind != "chat"));
    let p = rows_for(&pext(&ann.id)).into_iter().last().unwrap();
    assert_eq!(
        (p.op.as_str(), p.cause.as_str(), p.actor.as_str()),
        ("deprovision", "principal_delete", "user:boss")
    );
    assert_eq!(rows_for(&wext(&wid)).last().unwrap().op, "deprovision");
    drop_ws(&wid);
}

#[test]
fn principal_email_and_full_name_enqueue_update() {
    let _e = setup();
    enable_chat();
    let ann = person("ann");
    let bob = person("bob");
    grant("principal", "ann", None).unwrap();
    skin::set_principal_email_as("ann", Some("ann@example.com"), "owner-token").unwrap();
    let r = rows_for(&pext(&ann.id)).into_iter().last().unwrap();
    assert_eq!(
        (r.op.as_str(), r.cause.as_str()),
        ("update", "principal_update")
    );
    let p = payload(&r);
    assert_eq!(p["profile"]["email"], "ann@example.com");
    assert_eq!(p["profile"]["display_name"], "ann");
    assert!(p["profile"].get("title").is_none(), "title stays Keep");
    skin::set_principal_full_name_as("ann", Some("Ann Lee"), "owner-token").unwrap();
    let r = rows_for(&pext(&ann.id)).into_iter().last().unwrap();
    assert_eq!(payload(&r)["profile"]["display_name"], "Ann Lee");
    skin::set_principal_email("ann", None).unwrap();
    let r = rows_for(&pext(&ann.id)).into_iter().last().unwrap();
    assert!(
        payload(&r)["profile"]["email"].is_null(),
        "cleared email is an explicit null"
    );
    // No chat grant → nothing enqueued.
    skin::set_principal_full_name("bob", Some("Bob")).unwrap();
    assert!(rows_for(&pext(&bob.id)).is_empty());
}

#[test]
fn non_chat_grants_do_not_enqueue_and_keep_old_session_rules() {
    let _e = setup();
    enable_chat();
    let ann = person("ann");
    let before = rows().len();
    for kind in ["mailbox", "database", "cli"] {
        let g = skin::create_grant(&SkinGrantWrite {
            subject_kind: "principal".into(),
            subject_id: ann.id.clone(),
            kind: kind.into(),
            target_id: "t1".into(),
            role: None,
            scope: Some("x".into()),
            created_by: None,
        })
        .unwrap();
        skin::delete_grant_by_id(&g.id).unwrap();
    }
    assert_eq!(
        rows().len(),
        before,
        "non-hooked kinds never touch the outbox"
    );
}

#[test]
fn chat_grant_change_never_ends_k2_app_sessions() {
    let _e = setup();
    enable_chat();
    person("ann");
    skin::set_principal_password("ann", Some("pw-long-enough")).unwrap();
    let (_meta, raw) = skin::create_session_token("ann").unwrap();
    assert!(skin::resolve_skin_token(&raw).is_some(), "session works");
    let g = grant("principal", "ann", None).unwrap();
    skin::delete_grant_by_id(&g.id).unwrap();
    assert!(
        skin::resolve_skin_token(&raw).is_some(),
        "a chat grant is not a K2 login: deleting it must not log the guest out of their Apps (S24)"
    );
}

#[test]
fn disable_and_enable_chat_grant_enqueue_deprovision_and_provision() {
    let _e = setup();
    enable_chat();
    let ann = person("ann");
    let g = grant("principal", "ann", None).unwrap();
    deliver_all();
    skin::set_grant_enabled_as(&g.id, false, "user:boss").unwrap();
    let r = rows_for(&pext(&ann.id)).into_iter().last().unwrap();
    assert_eq!(
        (r.op.as_str(), r.cause.as_str()),
        ("deprovision", "grant_disable")
    );
    skin::set_grant_enabled_as(&g.id, true, "user:boss").unwrap();
    let r = rows_for(&pext(&ann.id)).into_iter().last().unwrap();
    assert_eq!(
        (r.op.as_str(), r.cause.as_str()),
        ("provision", "grant_enable")
    );
    // Re-enable needs the target on.
    skin::set_grant_enabled(&g.id, false).unwrap();
    set_target(CHAT_KIND, false, None, None, "owner-token").unwrap();
    assert_eq!(
        skin::set_grant_enabled(&g.id, true).unwrap_err(),
        CHAT_NOT_ENABLED
    );
}

// ── ordering, reserved ops, owner, targets ───────────────────────────

#[test]
fn due_heads_keep_subject_order_and_wait_for_the_person() {
    let _e = setup();
    enable_chat();
    let ann = person("ann");
    grant("principal", "ann", None).unwrap();
    let (wid, _) = add_ws("ord");
    grant("workspace", &wid, Some("assistant:ann")).unwrap();
    let due1: Vec<String> = due(CHAT_KIND, 100)
        .unwrap()
        .into_iter()
        .map(|r| r.subject_key)
        .collect();
    assert!(due1.contains(&"k2:owner".to_string()));
    assert!(due1.contains(&pext(&ann.id)));
    assert!(
        !due1.contains(&wext(&wid)),
        "assistant waits for its person: {due1:?}"
    );
    // Only the HEAD of a subject is due (owner provision, not owner_set).
    let owner_due: Vec<_> = due(CHAT_KIND, 100)
        .unwrap()
        .into_iter()
        .filter(|r| r.subject_key == "k2:owner")
        .collect();
    assert_eq!(owner_due.len(), 1);
    assert_eq!(owner_due[0].op, "provision");
    // Person blocked → assistant still waits; another subject unaffected.
    let person_row = rows_for(&pext(&ann.id)).remove(0);
    record_blocked(&person_row.id, Some(400), "validation_failed: x").unwrap();
    let due2: Vec<String> = due(CHAT_KIND, 100)
        .unwrap()
        .into_iter()
        .map(|r| r.subject_key)
        .collect();
    assert!(!due2.contains(&wext(&wid)));
    assert!(
        due2.contains(&"k2:owner".to_string()),
        "other subjects keep going"
    );
    // Retry → done → the assistant becomes due.
    retry(&person_row.id).unwrap();
    record_done(&person_row.id, 200, "{}").unwrap();
    let due3: Vec<String> = due(CHAT_KIND, 100)
        .unwrap()
        .into_iter()
        .map(|r| r.subject_key)
        .collect();
    assert!(due3.contains(&wext(&wid)), "{due3:?}");
    drop_ws(&wid);
}

fn revoke_rows() -> Vec<outbox::OutboxRow> {
    rows()
        .into_iter()
        .filter(|r| r.op == "sew_session_revoke")
        .collect()
}

/// S28 §12 wired to S27 (0.46.1): a skin logout and a password change of
/// a chat member queue `sew_session_revoke` on the chat outbox, each on
/// its own `revoke:…` subject, delivered even while the person's own
/// provisioning queue is blocked.
#[test]
fn session_revokes_ride_the_chat_outbox_on_their_own_subject() {
    let _e = setup();
    crate::sew_revoke::clear_sink();
    enable_chat();
    let ann = person("ann");
    skin::set_principal_password("ann", Some("s3cret-horse-battery")).unwrap();
    grant("principal", "ann", None).unwrap();
    let prov = rows_for(&pext(&ann.id))
        .into_iter()
        .find(|r| r.op == "provision")
        .expect("ann's provision");
    record_blocked(&prov.id, Some(400), "blocked (test)").unwrap();

    // Logout of one session → {k2_sid: skn_<id>}.
    let (meta, _raw) = skin::create_session_token("ann").expect("chat-only login");
    assert!(skin::revoke_token(&meta.id).unwrap());
    let rv = revoke_rows();
    assert_eq!(rv.len(), 1, "{rv:?}");
    let sid = format!("skn_{}", meta.id);
    assert_eq!(rv[0].subject_key, format!("revoke:k2_sid:{sid}"));
    assert_eq!(payload(&rv[0]), serde_json::json!({ "k2_sid": sid }));
    assert_eq!(rv[0].cause, "session_revoke");
    assert_eq!(rv[0].tool, CHAT_KIND);
    assert_eq!((rv[0].state.as_str(), rv[0].attempts), (STATE_PENDING, 0));
    assert!(rv[0].idem_key.starts_with("k2ob_"));

    // Due although ann's provision is blocked: a revoke is its own queue.
    let due_now = due(CHAT_KIND, 100).unwrap();
    assert!(due_now.iter().any(|r| r.id == rv[0].id), "{due_now:?}");
    assert!(due_now.iter().all(|r| r.id != prov.id));

    // A password change ends every session → {external_id}.
    skin::create_session_token("ann").expect("login again");
    skin::set_principal_password("ann", Some("another-horse-battery")).unwrap();
    let rv = revoke_rows();
    assert_eq!(rv.len(), 2, "{rv:?}");
    assert_eq!(rv[1].subject_key, format!("revoke:{}", pext(&ann.id)));
    assert_eq!(payload(&rv[1]), serde_json::json!({ "external_id": pext(&ann.id) }));
}

/// No chat target row (Sew never set up here): nothing is queued, and the
/// revoke itself still happens. With a target, a rolled-back write leaves
/// no row (same transaction).
#[test]
fn session_revoke_needs_a_chat_target_and_rolls_back_with_its_write() {
    let _e = setup();
    crate::sew_revoke::clear_sink();
    let bo = person("bo");
    skin::set_principal_password("bo", Some("s3cret-horse-battery")).unwrap();
    skin::insert_chat_grant_for_tests(&bo.id).unwrap();
    let (meta, raw) = skin::create_session_token("bo").expect("login");
    assert!(skin::revoke_token(&meta.id).unwrap());
    assert!(skin::resolve_skin_token(&raw).is_none(), "the K2 session ended");
    assert!(revoke_rows().is_empty(), "no target → no row");

    enable_chat();
    let op = crate::sew_revoke::SewSessionRevoke::K2Sid("skn_rolled_back".into());
    skin::with_skin_conn(|c| {
        let tx = c.unchecked_transaction().map_err(|e| e.to_string())?;
        crate::sew_revoke::enqueue(&tx, &op)?;
        // Dropped without commit: the write failed after the enqueue.
        drop(tx);
        Ok(())
    })
    .unwrap();
    assert!(revoke_rows().is_empty(), "rolled back with its write");
    skin::with_skin_conn(|c| {
        let tx = c.unchecked_transaction().map_err(|e| e.to_string())?;
        crate::sew_revoke::enqueue(&tx, &op)?;
        tx.commit().map_err(|e| e.to_string())
    })
    .unwrap();
    assert_eq!(revoke_rows().len(), 1);
}

#[test]
fn target_enable_enqueues_owner_once_and_owner_state_tracks_it() {
    let _e = setup();
    assert_eq!(owner_state(CHAT_KIND).unwrap(), OwnerState::None);
    let t = enable_chat();
    assert!(t.enabled);
    assert_eq!(t.socket, "/tmp/k2-test-sew.sock");
    assert!(reconcile_requested(), "enable asks for a sweep");
    let owner = rows_for("k2:owner");
    let ops: Vec<&str> = owner.iter().map(|r| r.op.as_str()).collect();
    assert_eq!(ops, ["provision", "owner_set"]);
    let p = payload(&owner[0]);
    assert_eq!(
        (p["profile"]["username"].as_str(), p["guest"].as_bool()),
        (Some("owner"), Some(false))
    );
    assert_eq!(
        payload(&owner[1]),
        serde_json::json!({"external_id":"k2:owner","force":false})
    );
    assert_eq!(owner_state(CHAT_KIND).unwrap(), OwnerState::Pending);
    // Enabling again is not a transition: nothing new.
    enable_chat();
    assert_eq!(rows_for("k2:owner").len(), 2);
    record_done(&owner[0].id, 200, "{}").unwrap();
    record_blocked(&owner[1].id, Some(422), "invalid_state: x").unwrap();
    assert_eq!(owner_state(CHAT_KIND).unwrap(), OwnerState::Blocked);
    retry(&owner[1].id).unwrap();
    record_done(&owner[1].id, 200, "{}").unwrap();
    assert_eq!(owner_state(CHAT_KIND).unwrap(), OwnerState::Set);
}

#[test]
fn retry_and_skip_rules_a11() {
    let _e = setup();
    enable_chat();
    let ann = person("ann");
    grant("principal", "ann", None).unwrap();
    let r = rows_for(&pext(&ann.id)).remove(0);
    let e = retry(&r.id).unwrap_err();
    assert!(
        e.contains("retry needs a blocked row or a pending row that has failed"),
        "{e}"
    );
    record_retry(&r.id, Some(503), "unavailable: x", 600).unwrap();
    let again = retry(&r.id).expect("pending that failed may be retried (A11)");
    assert!(again.next_attempt_at <= outbox::now_secs());
    assert_eq!(again.attempts, 1, "attempts kept");
    assert!(skip(&r.id)
        .unwrap_err()
        .contains("only a blocked row can be skipped"));
    record_blocked(&r.id, Some(400), "validation_failed: x").unwrap();
    let s = skip(&r.id).unwrap();
    assert_eq!(s.state, "skipped");
    assert!(reconcile_requested(), "skip asks reconcile to repair");
    assert!(retry(&r.id).is_err(), "a skipped row is final");
}

#[test]
fn drop_target_grants_deletes_grants_and_skips_open_rows() {
    let _e = setup();
    enable_chat();
    person("ann");
    let g = grant("principal", "ann", None).unwrap();
    let report = drop_target_grants(CHAT_KIND).unwrap();
    assert_eq!(report.grants_deleted, 1);
    assert!(
        report.rows_skipped >= 3,
        "owner provision + owner_set + ann: {report:?}"
    );
    assert!(skin::list_grants().unwrap().iter().all(|x| x.id != g.id));
    assert!(rows().iter().all(|r| r.state == "skipped"));
}

#[test]
fn old_skin_db_gains_hook_tables_and_keeps_rows() {
    let home = crate::test_env::TempHome::new();
    let path = crate::paths::k2_home().join("skin.db");
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE principals (id TEXT PRIMARY KEY, username TEXT NOT NULL UNIQUE, created_at INTEGER NOT NULL);
             INSERT INTO principals VALUES ('0f8fad5b-d9cb-469f-a165-70867728950e', 'old', 1);",
        )
        .unwrap();
    }
    let names = skin::list_principals().unwrap();
    assert!(names.iter().any(|p| p.username == "old"), "old row kept");
    let c = rusqlite::Connection::open(&path).unwrap();
    for t in ["tool_outbox", "tool_targets"] {
        let n: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                rusqlite::params![t],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "{t} created");
    }
    let v: i64 = c
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        v,
        skin::SKIN_SCHEMA_VERSION,
        "no ladder step taken (4 stays reserved for R3)"
    );
    drop(home);
}

// ── reconcile ────────────────────────────────────────────────────────

fn uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn profile(username: &str, display: &str) -> ProfileInput {
    ProfileInput {
        display_name: display.into(),
        username: username.into(),
        email: None,
        title: None,
        pronouns: None,
    }
}

fn desired_person(id: &str, username: &str, guest: bool, rev: &str) -> Desired {
    Desired {
        request: ProvisionRequest {
            external_id: ExternalId::K2Principal(id.into()),
            kind: MemberKind::Person,
            agent_kind: None,
            assistant_of: None,
            guest,
            profile: profile(username, username),
        },
        grant: None,
        grant_rev: rev.into(),
    }
}

fn desired_agent(id: &str, username: &str, kind: AgentKind, of: Option<ExternalId>) -> Desired {
    Desired {
        request: ProvisionRequest {
            external_id: ExternalId::K2Workspace(id.into()),
            kind: MemberKind::Agent,
            agent_kind: Some(kind),
            assistant_of: of,
            guest: false,
            profile: profile(username, username),
        },
        grant: None,
        grant_rev: format!("g-{id}"),
    }
}

fn member(ext: ExternalId, kind: MemberKind, username: &str, display: &str) -> MgmtPrincipal {
    MgmtPrincipal {
        member_id: format!("{}", username.len() + 100),
        external_id: ext,
        kind,
        agent_kind: None,
        assistant_of: None,
        install_role: InstallRole::Member,
        username: username.into(),
        display_name: display.into(),
        deactivated: false,
        created_at: "2026-10-15T00:00:00Z".into(),
        deactivated_at: None,
        email: None,
        title: None,
        pronouns: None,
    }
}

fn ops_for(planned: &[reconcile::PlannedOp], ext: &ExternalId) -> Vec<Op> {
    planned
        .iter()
        .filter(|p| &p.subject == ext)
        .map(|p| p.op)
        .collect()
}

#[test]
fn reconcile_plan_table() {
    let (a, b, c, d, e, f, x, y) = (
        uuid(),
        uuid(),
        uuid(),
        uuid(),
        uuid(),
        uuid(),
        uuid(),
        uuid(),
    );
    let pa = ExternalId::K2Principal(a.clone());
    let pb = ExternalId::K2Principal(b.clone());
    let pc = ExternalId::K2Principal(c.clone());
    let pd = ExternalId::K2Principal(d.clone());
    let we = ExternalId::K2Workspace(e.clone());
    let wf = ExternalId::K2Workspace(f.clone());
    let px = ExternalId::K2Principal(x.clone());
    let wy = ExternalId::K2Workspace(y.clone());
    let owner = Desired {
        request: ProvisionRequest {
            external_id: ExternalId::K2Owner,
            kind: MemberKind::Person,
            agent_kind: None,
            assistant_of: None,
            guest: false,
            profile: profile("owner", "Owner"),
        },
        grant: None,
        grant_rev: OWNER_GRANT_REV.into(),
    };
    let desired = vec![
        owner,
        desired_person(&a, "absent", false, "ra"), // absent → provision
        desired_person(&b, "deact", false, "rb"),  // deactivated → provision
        desired_person(&c, "guesty", true, "rc"),  // guest differs → provision
        desired_person(&d, "named", false, "rd"),  // display differs → update
        desired_agent(&e, "agente", AgentKind::Coworker, None), // agent kind differs → deprovision+provision
        desired_agent(&f, "agentf", AgentKind::Assistant, Some(pa.clone())), // pin differs → provision
    ];
    let mut owner_m = member(ExternalId::K2Owner, MemberKind::Person, "owner", "Owner");
    owner_m.install_role = InstallRole::Owner;
    let mut mb = member(pb.clone(), MemberKind::Person, "deact", "deact");
    mb.deactivated = true;
    let mc = member(pc.clone(), MemberKind::Person, "guesty", "guesty"); // member, wants guest
    let md = member(pd.clone(), MemberKind::Person, "named", "Wrong Name");
    let mut me = member(we.clone(), MemberKind::Agent, "agente", "agente");
    me.agent_kind = Some(AgentKind::Assistant);
    me.assistant_of = Some(pa.clone());
    let mut mf = member(wf.clone(), MemberKind::Agent, "agentf", "agentf");
    mf.agent_kind = Some(AgentKind::Assistant);
    mf.assistant_of = Some(pb.clone());
    let mx = member(px.clone(), MemberKind::Person, "extra", "extra"); // not desired → deprovision
    let mut my = member(wy.clone(), MemberKind::Agent, "gone", "gone");
    my.deactivated = true; // not desired but already deactivated → nothing
    let local = member(
        ExternalId::Local("solo".into()),
        MemberKind::Person,
        "solo",
        "solo",
    );
    let other = member(
        ExternalId::Other("link:4242:AB3DEFGHIJKLMNOP".into()),
        MemberKind::Person,
        "conn",
        "conn",
    );
    let actual = vec![owner_m, mb, mc, md, me, mf, mx, my, local, other];
    let (planned, s) = reconcile::plan(&desired, &actual, &HashSet::new(), &HashMap::new());
    assert_eq!(
        ops_for(&planned, &ExternalId::K2Owner),
        Vec::<Op>::new(),
        "owner: never deprovision / owner_set"
    );
    assert_eq!(ops_for(&planned, &pa), [Op::Provision]);
    assert_eq!(ops_for(&planned, &pb), [Op::Provision]);
    assert_eq!(ops_for(&planned, &pc), [Op::Provision]);
    assert_eq!(ops_for(&planned, &pd), [Op::Update]);
    assert_eq!(ops_for(&planned, &we), [Op::Deprovision, Op::Provision]);
    assert_eq!(ops_for(&planned, &wf), [Op::Provision]);
    assert_eq!(ops_for(&planned, &px), [Op::Deprovision]);
    assert_eq!(ops_for(&planned, &wy), Vec::<Op>::new());
    let upd = planned.iter().find(|p| p.subject == pd).unwrap();
    assert_eq!(
        upd.payload["profile"],
        serde_json::json!({"display_name":"named"})
    );
    let fprov = planned.iter().find(|p| p.subject == wf).unwrap();
    assert_eq!(
        fprov.after_subject.as_deref(),
        Some(pa.to_string().as_str())
    );
    assert_eq!((s.ignored_local, s.ignored_other), (1, 1));
    assert_eq!((s.provisioned, s.updated, s.deprovisioned), (5, 1, 2));
}

/// K-M6 (Meet): reconcile deprovisions only the members K2 owns
/// (`k2:principal:*`, `k2:workspace:*`, `k2:connect:*`). A Meet link guest
/// (`link:<channel>:<base32>`, install role `link_guest`, Sew CC2) is
/// never K2's: Sew hides it from the list by default (CC7), and if a list
/// carries one anyway (`?include=link_guests`, a newer Sew), it parses
/// leniently and is ignored, never deprovisioned. A `k2:connect:*` member
/// IS K2's (SW2): with no `connect` grant it is deprovisioned like a
/// principal. The lenient parse must survive the K1 swap to the vendored
/// sew-proto (its `ExternalId` has no `link:` form).
#[test]
fn reconcile_never_deprovisions_meet_link_guests_k_m6() {
    let raw = r#"{"items":[
        {"member_id":"501","external_id":"link:4242:AB3DEFGHIJKLMNOP","kind":"person",
         "install_role":"link_guest","username":"g-ab3defgh","display_name":"Guest Ana",
         "deactivated":false,"created_at":"2026-10-17T00:00:00Z","meeting_id":"77"},
        {"member_id":"502","external_id":"k2:connect:2b0c8f1e-9a43-4d55-8f7e-1c2d3e4f5a6b","kind":"person",
         "install_role":"member","username":"rosson","display_name":"Rosson",
         "deactivated":false,"created_at":"2026-10-17T00:00:00Z"},
        {"member_id":"503","external_id":"k2:principal:0f6a3c1d-5b2e-4c7a-9d8e-112233445566","kind":"person",
         "install_role":"member","username":"left","display_name":"Left",
         "deactivated":false,"created_at":"2026-10-17T00:00:00Z"}
    ],"has_more":false}"#;
    let page: sew_wire::MgmtPrincipalPage =
        serde_json::from_str(raw).expect("a list with link guests still parses");
    let link = &page.items[0];
    assert_eq!(
        link.external_id,
        ExternalId::Other("link:4242:AB3DEFGHIJKLMNOP".into())
    );
    assert!(!link.external_id.is_known());
    assert_eq!(link.install_role, InstallRole::Unknown);
    let connect = ExternalId::K2Connect("2b0c8f1e-9a43-4d55-8f7e-1c2d3e4f5a6b".into());
    assert_eq!(page.items[1].external_id, connect);
    let left = ExternalId::K2Principal("0f6a3c1d-5b2e-4c7a-9d8e-112233445566".into());
    assert_eq!(page.items[2].external_id, left);

    // Nothing is desired: the K2 principal and the Connect login (no
    // grant) go; the link guest is counted and left alone.
    let (planned, s) = reconcile::plan(&[], &page.items, &HashSet::new(), &HashMap::new());
    assert_eq!(ops_for(&planned, &left), [Op::Deprovision]);
    assert_eq!(ops_for(&planned, &connect), [Op::Deprovision]);
    assert_eq!(
        planned
            .iter()
            .filter(|p| p.subject.to_string().starts_with("link:")
                || p.payload.to_string().contains("link:"))
            .count(),
        0,
        "never an op for a link guest: {planned:?}"
    );
    assert_eq!(planned.len(), 2, "{planned:?}");
    assert_eq!((s.deprovisioned, s.ignored_other), (2, 1));

    // Busy or not, a link guest is never K2's to remove.
    let busy: HashSet<String> = ["link:4242:AB3DEFGHIJKLMNOP".to_string()].into();
    let (planned, s) = reconcile::plan(&[], &page.items, &busy, &HashMap::new());
    assert_eq!(planned.len(), 2);
    assert_eq!((s.skipped_busy, s.ignored_other), (0, 1));
}

#[test]
fn reconcile_accepts_suffixed_username_and_admin_role_is_not_drift() {
    let a = uuid();
    let pa = ExternalId::K2Principal(a.clone());
    let d = desired_person(&a, "sales", false, "r");
    let mut m = member(
        pa.clone(),
        MemberKind::Person,
        &chat::suffixed_username("sales", &pa.to_string()),
        "sales",
    );
    m.install_role = InstallRole::Admin;
    let (planned, _) = reconcile::plan(&[d], &[m], &HashSet::new(), &HashMap::new());
    assert!(planned.is_empty(), "{planned:?}");
}

#[test]
fn reconcile_skips_busy_subjects() {
    let (a, x) = (uuid(), uuid());
    let pa = ExternalId::K2Principal(a.clone());
    let px = ExternalId::K2Principal(x.clone());
    let busy: HashSet<String> = [pa.to_string(), px.to_string()].into();
    let (planned, s) = reconcile::plan(
        &[desired_person(&a, "ann", false, "r")],
        &[member(px, MemberKind::Person, "x", "x")],
        &busy,
        &HashMap::new(),
    );
    assert!(planned.is_empty(), "{planned:?}");
    assert_eq!(s.skipped_busy, 2);
}

#[test]
fn username_suffix_is_deterministic_and_fits_64() {
    let ext = "k2:principal:0f8fad5b-d9cb-469f-a165-70867728950e";
    let a = chat::suffixed_username("sales", ext);
    assert_eq!(a, chat::suffixed_username("sales", ext));
    assert!(a.starts_with("sales-") && a.len() == 10, "{a}");
    assert!(sew_wire::is_valid_handle(&a));
    let long = chat::suffixed_username(&"z".repeat(64), ext);
    assert_eq!(long.len(), 64);
    assert!(sew_wire::is_valid_handle(&long));
    assert_ne!(a, chat::suffixed_username("sales", "k2:owner"));
    assert_eq!(chat::owner_username(Some("!")), "owner");
    assert_eq!(chat::owner_username(None), "owner");
}

// ── k2:owner username = the linked K2 login (Sew, 2026-10-17) ────────

#[test]
fn owner_username_is_the_login_normalised() {
    assert_eq!(chat::owner_username(Some("z3thon")), "z3thon");
    // Dots → `-`, other characters stripped, lowercased.
    assert_eq!(chat::owner_username(Some("rosson.long")), "rosson-long");
    assert_eq!(chat::owner_username(Some("Ro$s.On_2 ")), "ros-on_2");
    assert_eq!(chat::owner_username(Some("ünï.x9")), "n-x9");
    assert_eq!(chat::login_username(&"a".repeat(80)).unwrap().len(), 64);
    // Nothing usable left → `owner`.
    assert_eq!(chat::owner_username(Some("é")), "owner");
    assert_eq!(chat::owner_username(Some("a")), "owner");
    for l in ["z3thon", "rosson.long", "Ro$s.On_2", "ALL", "here"] {
        let u = chat::login_username(l).unwrap();
        chat::check_username(&u).unwrap_or_else(|e| panic!("{l} → {u}: {e}"));
    }
}

#[test]
fn owner_username_avoids_reserved_mentions() {
    assert_eq!(
        chat::RESERVED_USERNAMES,
        ["here", "channel", "everyone", "all"]
    );
    for (login, want) in [
        ("here", "here-k2"),
        ("channel", "channel-k2"),
        ("Everyone", "everyone-k2"),
        ("all", "all-k2"),
        ("a.ll", "a-ll"),
        ("allison", "allison"),
    ] {
        assert_eq!(chat::owner_username(Some(login)), want, "{login}");
    }
    // The no-account fallback is not reserved.
    assert!(!chat::RESERVED_USERNAMES.contains(&"owner"));
    chat::check_username("owner").unwrap();
    // A reserved word never reaches Sew from a grant either.
    for w in chat::RESERVED_USERNAMES {
        let e = chat::check_username(w).unwrap_err();
        assert!(e.contains("reserved chat mention"), "{e}");
    }
}

fn owner_desired(username: &str) -> Desired {
    Desired {
        request: ProvisionRequest {
            external_id: ExternalId::K2Owner,
            kind: MemberKind::Person,
            agent_kind: None,
            assistant_of: None,
            guest: false,
            profile: profile(username, "Rosson Long"),
        },
        grant: None,
        grant_rev: OWNER_GRANT_REV.into(),
    }
}

fn owner_member(username: &str) -> MgmtPrincipal {
    let mut m = member(ExternalId::K2Owner, MemberKind::Person, username, "Rosson Long");
    m.install_role = InstallRole::Owner;
    m
}

fn username_updates(planned: &[reconcile::PlannedOp]) -> Vec<String> {
    planned
        .iter()
        .filter(|p| p.op == Op::Update)
        .filter_map(|p| p.payload["profile"]["username"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn reconcile_renames_an_existing_owner_member_to_the_login() {
    // 0.46.0-era install: Sew holds the display-name slug.
    let (planned, s) = reconcile::plan(
        &[owner_desired("z3thon")],
        &[owner_member("rosson-long")],
        &HashSet::new(),
        &HashMap::new(),
    );
    assert_eq!(planned.len(), 1, "{planned:?}");
    assert_eq!(planned[0].op, Op::Update);
    assert_eq!(planned[0].subject, ExternalId::K2Owner);
    assert_eq!(username_updates(&planned), ["z3thon"]);
    assert_eq!(planned[0].payload["profile"].get("display_name"), None);
    assert_eq!(s.updated, 1);
}

#[test]
fn owner_fallback_username_is_sticky_until_the_login_changes() {
    let fallback = chat::suffixed_username("z3thon", "k2:owner");
    let mut sew = owner_member(&fallback);
    // Two reconciles after a fallback: no rename.
    for _ in 0..2 {
        let (planned, _) = reconcile::plan(
            &[owner_desired("z3thon")],
            std::slice::from_ref(&sew),
            &HashSet::new(),
            &HashMap::new(),
        );
        assert!(planned.is_empty(), "{planned:?}");
    }
    // A re-provision of the member keeps the fallback, too.
    let mut req = owner_desired("z3thon").request;
    assert!(chat::fill_provision_from_member(&mut req, &sew));
    assert_eq!(req.profile.username, fallback);
    // A bare-login member is left as it is.
    let mut req = owner_desired("z3thon").request;
    assert!(!chat::fill_provision_from_member(&mut req, &owner_member("z3thon")));
    assert_eq!(req.profile.username, "z3thon");

    // The login changes: exactly one rename, then nothing.
    let (planned, _) = reconcile::plan(
        &[owner_desired("rosson")],
        std::slice::from_ref(&sew),
        &HashSet::new(),
        &HashMap::new(),
    );
    assert_eq!(username_updates(&planned), ["rosson"]);
    assert_eq!(planned.len(), 1);
    sew.username = "rosson".into();
    let (planned, _) = reconcile::plan(
        &[owner_desired("rosson")],
        std::slice::from_ref(&sew),
        &HashSet::new(),
        &HashMap::new(),
    );
    assert!(planned.is_empty(), "{planned:?}");
    // A different login's fallback is not "sticky" for the new login.
    let mut req = owner_desired("rosson").request;
    assert!(!chat::fill_provision_from_member(&mut req, &owner_member(&fallback)));
    assert_eq!(req.profile.username, "rosson");
}

// ── K4: apply_reconcile against skin.db ─────────────────────────────

#[test]
fn apply_reconcile_removes_orphans_and_provisions_drift() {
    let _e = setup();
    enable_chat();
    deliver_all();
    let ann = person("ann");
    let g = grant("principal", "ann", None).unwrap();
    let (wid, handle) = add_ws("gone");
    let wg = grant("workspace", &wid, Some("coworker")).unwrap();
    deliver_all();
    // The workspace is deleted in the main DB (no shared transaction).
    drop_ws(&wid);
    // Sew is empty except an owner: ann must be provisioned again.
    let mut owner_m = member(ExternalId::K2Owner, MemberKind::Person, "owner", "Owner");
    owner_m.install_role = InstallRole::Owner;
    let mut ws_m = member(
        ExternalId::K2Workspace(wid.clone()),
        MemberKind::Agent,
        &handle,
        "x",
    );
    ws_m.agent_kind = Some(AgentKind::Coworker);
    let s = apply_reconcile(CHAT_KIND, &[owner_m.clone(), ws_m]).unwrap();
    assert_eq!(s.orphans_removed, 1);
    assert!(
        skin::list_grants().unwrap().iter().all(|x| x.id != wg.id),
        "orphan grant deleted"
    );
    let w = rows_for(&wext(&wid)).into_iter().last().unwrap();
    assert_eq!(
        (w.op.as_str(), w.cause.as_str(), w.actor.as_str()),
        ("deprovision", "workspace_removed", ACTOR_RECONCILE)
    );
    let a = rows_for(&pext(&ann.id)).into_iter().last().unwrap();
    assert_eq!(
        (a.op.as_str(), a.cause.as_str()),
        ("provision", "reconcile")
    );
    assert_eq!(a.grant_rev.as_deref(), Some(grant_rev(&g).as_str()));
    let t = get_target(CHAT_KIND).unwrap().unwrap();
    assert!(t.last_reconcile_at.is_some());
    assert_eq!(t.last_reconcile.unwrap()["orphansRemoved"], 1);
    // A second sweep while those rows are open plans nothing new for them.
    // (The owner is in Sew, so only busy subjects are left to consider.)
    let n = rows().len();
    apply_reconcile(CHAT_KIND, &[owner_m]).unwrap();
    assert_eq!(rows().len(), n, "busy subjects are never re-planned");
}

#[test]
fn hc2_held_subject_is_not_re_added_until_the_grant_changes() {
    let _e = setup();
    enable_chat();
    deliver_all();
    let ann = person("ann");
    let g = grant("principal", "ann", None).unwrap();
    let r = rows_for(&pext(&ann.id)).remove(0);
    record_held(
        &r.id,
        200,
        r#"{"member_id":"9","skipped":["9"]}"#,
        "skipped by Sew (banned)",
    )
    .unwrap();
    assert_eq!(row(&r.id).state, "held");
    let mut owner_m = member(ExternalId::K2Owner, MemberKind::Person, "owner", "Owner");
    owner_m.install_role = InstallRole::Owner;
    // Sew has no active member for ann; reconcile must NOT re-add her.
    let s = apply_reconcile(CHAT_KIND, &[owner_m.clone()]).unwrap();
    assert_eq!(s.skipped_held, 1);
    assert_eq!(
        rows_for(&pext(&ann.id)).len(),
        1,
        "no re-add while the grant is unchanged"
    );
    let states = hook_states(&skin::list_grants().unwrap()).unwrap();
    assert_eq!(states.get(&g.id).map(|s| s.0.as_str()), Some("held"));
    assert_eq!(summary_held(), 1);
    // The grant changes (member → guest): the hook enqueues again at once,
    // and reconcile no longer holds the subject under the new rev.
    skin::update_grant_scope(&g.id, "guest", "owner-token").unwrap();
    let last = rows_for(&pext(&ann.id)).into_iter().last().unwrap();
    assert_eq!(
        (last.op.as_str(), last.state.as_str()),
        ("provision", STATE_PENDING)
    );
    record_done(&last.id, 200, "{}").unwrap();
    let s2 = apply_reconcile(CHAT_KIND, &[owner_m]).unwrap();
    assert_eq!(s2.skipped_held, 0);
    assert_eq!(
        s2.provisioned, 1,
        "absent in Sew under a new grant rev → provision"
    );
}

fn summary_held() -> i64 {
    skin::with_skin_conn(|c| outbox::summary(c, outbox::now_secs()))
        .unwrap()
        .held
}

/// SW2: a `connect` chat grant names a Connect login by username or id,
/// stores its id, and provisions `k2:connect:<id>` with the login as the
/// username. `connect` is a chat-only subject; an unknown login is refused.
#[test]
fn connect_chat_grant_stores_the_id_and_provisions_the_login() {
    let _e = setup();
    enable_chat();
    crate::connect_users::add_user_with_names("bob", "pw-bob-long", Some("Bob"), Some("Stone"))
        .unwrap();
    let id = crate::connect_users::user_uuid("bob").unwrap().unwrap();
    let w = |subject: &str, kind: &str, scope: Option<&str>| SkinGrantWrite {
        subject_kind: "connect".into(),
        subject_id: subject.into(),
        kind: kind.into(),
        target_id: if kind == "app" { "svc-x".into() } else { String::new() },
        role: None,
        scope: scope.map(str::to_string),
        created_by: Some("owner-token".into()),
    };
    assert_eq!(
        skin::create_grant(&w("nobody", "chat", None)).unwrap_err(),
        "unknown Connect user 'nobody'"
    );
    assert_eq!(
        skin::create_grant(&w("bob", "app", Some("[]"))).unwrap_err(),
        "subjectKind connect is only for kind=chat"
    );
    assert_eq!(
        skin::create_grant(&w("bob", "chat", Some("coworker"))).unwrap_err(),
        chat::SCOPE_ERR
    );
    let g = skin::create_grant(&w("bob", "chat", Some("guest"))).unwrap();
    assert_eq!((g.subject_id.as_str(), g.scope.as_deref()), (id.as_str(), Some("guest")));
    let r = rows_for(&format!("k2:connect:{id}")).remove(0);
    assert_eq!(r.op, "provision");
    let p = payload(&r);
    assert_eq!(p["kind"], "person");
    assert_eq!(p["guest"], true);
    assert_eq!(p["profile"]["username"], "bob");
    assert_eq!(p["profile"]["display_name"], "Bob Stone");
    // By id works too (same grant → refused as a duplicate).
    assert_eq!(
        skin::create_grant(&w(&id, "chat", None)).unwrap_err(),
        "grant already exists for this subject, kind, and target"
    );
}

#[test]
fn rev147_connect_owners_and_admins_are_never_provisioned_by_role() {
    let _e = setup();
    crate::connect_users::add_user("boss", "pw-boss-long").unwrap();
    crate::connect_users::set_role("boss", crate::connect_users::Role::Owner).unwrap();
    crate::connect_users::add_user("adm", "pw-adm-long").unwrap();
    crate::connect_users::set_role("adm", crate::connect_users::Role::Admin).unwrap();
    enable_chat();
    let ann = person("ann");
    // An Owner-role Connect session grants chat to a principal: only that
    // principal (and the box owner) become members; `user:boss` is the
    // actor (attribution), never a subject.
    skin::create_grant(&SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: "ann".into(),
        kind: "chat".into(),
        target_id: String::new(),
        role: None,
        scope: None,
        created_by: Some("user:boss".into()),
    })
    .unwrap();
    // A Connect user cannot be a chat subject at all.
    let e = skin::create_grant(&SkinGrantWrite {
        subject_kind: "connect_user".into(),
        subject_id: "boss".into(),
        kind: "chat".into(),
        target_id: String::new(),
        role: None,
        scope: None,
        created_by: Some("user:boss".into()),
    })
    .unwrap_err();
    assert_eq!(e, "subjectKind must be principal, workspace or connect");
    // App user changes (an App grant for a Connect-named principal) never
    // provision anyone by themselves.
    let subjects: HashSet<String> = rows().into_iter().map(|r| r.subject_key).collect();
    assert_eq!(
        subjects,
        HashSet::from(["k2:owner".to_string(), pext(&ann.id)])
    );
    let ann_row = rows_for(&pext(&ann.id)).remove(0);
    assert_eq!(ann_row.actor, "user:boss");
    // Reconcile's desired set: exactly k2:owner + the granted principal.
    deliver_all();
    let s = apply_reconcile(CHAT_KIND, &[]).unwrap();
    assert_eq!(s.provisioned, 2, "{s:?}");
    let planned: HashSet<String> = rows()
        .into_iter()
        .filter(|r| r.cause == "reconcile")
        .map(|r| r.subject_key)
        .collect();
    assert_eq!(
        planned,
        HashSet::from(["k2:owner".to_string(), pext(&ann.id)])
    );
    for r in rows() {
        assert!(
            !r.subject_key.contains("boss") && !r.subject_key.contains("adm"),
            "{r:?}"
        );
        assert!(
            r.subject_key == "k2:owner" || r.subject_key.starts_with("k2:principal:"),
            "{r:?}"
        );
    }
}

#[test]
fn kick_reconcile_fires_on_main_db_and_app_user_changes() {
    let _e = setup();
    let (wid, _) = add_ws("kick");
    let _ = take_reconcile_request();
    crate::projects_ops::projects_delete(&wid).unwrap();
    assert!(take_reconcile_request(), "project delete (S27-A2)");
    person("ann");
    let _ = take_reconcile_request();
    skin::set_host_login("ann", false).unwrap();
    assert!(take_reconcile_request(), "App user change (host login off)");
    skin::remove_principal("ann").unwrap();
    assert!(take_reconcile_request(), "principal removal");
    crate::connect_users::add_user("rosson", "password1").expect("add login");
    let _ = take_reconcile_request();
    crate::app_settings::update(serde_json::json!({ "ownerAccounts": ["rosson"] })).expect("link");
    assert!(take_reconcile_request(), "owner account link");
    crate::connect_users::set_names("rosson", Some("Rosson"), Some("Long")).expect("names");
    assert!(take_reconcile_request(), "owner name change (linked account First/Last)");
}

// ── owner profile name (0.46.0 person names) ─────────────────────────

/// The `k2:owner` Sew profile's display name is the owner person's
/// displayName (the linked account's "First Last"); its username is the
/// linked K2 login, never a slug of that name.
#[test]
fn owner_profile_name_is_the_linked_accounts_first_last() {
    let _e = setup();
    crate::connect_users::add_user_with_names("z3thon", "password1", Some("Rosson"), Some("Long"))
        .expect("add login");
    crate::app_settings::update(serde_json::json!({ "ownerAccounts": ["z3thon"] })).expect("link");
    // The retired "Your name" setting is ignored even when set.
    crate::app_settings::update(serde_json::json!({ "ownerDisplayName": "Old Name" })).expect("legacy");
    let ctx = HookCtx::load("owner-token", &[]);
    assert_eq!(ctx.owner_name.as_deref(), Some("Rosson Long"));
    assert_eq!(ctx.owner_login.as_deref(), Some("z3thon"));
    let req = chat::owner_provision_request(&ctx);
    assert_eq!(req.external_id, ExternalId::K2Owner);
    assert_eq!(req.kind, MemberKind::Person);
    assert!(!req.guest);
    assert_eq!(req.profile.display_name, "Rosson Long");
    assert_eq!(req.profile.username, "z3thon");
    assert_eq!(req.profile.email, None);
}

/// A linked login that is a reserved word is moved off it; with no
/// linked account the username is `owner`.
#[test]
fn owner_username_from_disk_reserved_login_and_no_account() {
    let _e = setup();
    let ctx = HookCtx::load("owner-token", &[]);
    assert_eq!(ctx.owner_login, None);
    assert_eq!(chat::owner_provision_request(&ctx).profile.username, "owner");
    crate::connect_users::add_user("all", "password1").expect("add login");
    crate::app_settings::update(serde_json::json!({ "ownerAccounts": ["all"] })).expect("link");
    let req = chat::owner_provision_request(&HookCtx::load("owner-token", &[]));
    assert_eq!(req.profile.username, "all-k2");
    sew_wire::validate_provision(&req).expect("valid provision");
    // Removing the linked login falls back to `owner` and kicks a reconcile.
    let _ = take_reconcile_request();
    crate::connect_users::remove_user("all").expect("remove login");
    assert!(take_reconcile_request(), "linked login removal");
    let req = chat::owner_provision_request(&HookCtx::load("owner-token", &[]));
    assert_eq!(req.profile.username, "owner");
}

/// No First/Last: the linked username, and with nothing linked `Owner`
/// (username `owner`) — never the retired "Your name" setting.
#[test]
fn owner_profile_name_falls_back_to_username_then_owner() {
    let _e = setup();
    crate::app_settings::update(serde_json::json!({ "ownerDisplayName": "Old Name" })).expect("legacy");
    let ctx = HookCtx::load("owner-token", &[]);
    assert_eq!(ctx.owner_name.as_deref(), Some("Owner"));
    let req = chat::owner_provision_request(&ctx);
    assert_eq!(req.profile.display_name, "Owner");
    assert_eq!(req.profile.username, "owner");
    // `Default` (no names loaded) matches the no-account fallback.
    let req = chat::owner_provision_request(&HookCtx::default());
    assert_eq!((req.profile.display_name.as_str(), req.profile.username.as_str()), ("Owner", "owner"));

    crate::connect_users::add_user("z3thon", "password1").expect("add login");
    crate::app_settings::update(serde_json::json!({ "ownerAccounts": ["z3thon"] })).expect("link");
    let req = chat::owner_provision_request(&HookCtx::load("owner-token", &[]));
    assert_eq!(req.profile.display_name, "z3thon");
    assert_eq!(req.profile.username, "z3thon");
}

#[test]
fn sew_actor_header_is_sanitized_attribution() {
    assert_eq!(sew_actor_header("user:boss"), "user:boss");
    assert_eq!(sew_actor_header("user:a b/c"), "user:a_b_c");
    assert_eq!(sew_actor_header(""), ACTOR_SYSTEM);
    assert_eq!(sew_actor_header(&"x".repeat(300)).len(), 128);
    assert!(sew_wire::is_sew_actor(&sew_actor_header("agent:ünï")));
}

#[test]
fn outbox_statuses_and_prune_keep_open_and_held_rows() {
    let _e = setup();
    enable_chat();
    let ann = person("ann");
    grant("principal", "ann", None).unwrap();
    let all = rows();
    for r in &all {
        record_done(&r.id, 200, "{}").unwrap();
    }
    let a = rows_for(&pext(&ann.id)).remove(0);
    assert_eq!(a.state, STATE_DONE);
    // Age everything far past retention, then hold/open one row each.
    skin::with_skin_conn(|c| {
        c.execute("UPDATE tool_outbox SET done_at = 1, updated_at = 1", [])
            .map_err(|e| e.to_string())
    })
    .unwrap();
    skin::set_principal_full_name("ann", Some("Ann")).unwrap();
    let open = rows_for(&pext(&ann.id)).into_iter().last().unwrap();
    record_blocked(&open.id, Some(400), "x").unwrap();
    skin::with_skin_conn(|c| {
        c.execute(
            "UPDATE tool_outbox SET updated_at = 1 WHERE id = ?1",
            rusqlite::params![open.id],
        )
        .map_err(|e| e.to_string())
    })
    .unwrap();
    let pruned = prune().unwrap();
    assert_eq!(pruned, all.len(), "finished rows past 90 days go");
    let left = rows();
    assert_eq!(left.len(), 1);
    assert_eq!(
        left[0].state, STATE_BLOCKED,
        "blocked rows are never pruned"
    );
}

// ── A8: provision is declarative; K2 round-trips what it does not own ──

fn with_profile(
    mut m: MgmtPrincipal,
    email: Option<Option<&str>>,
    title: Option<Option<&str>>,
    pronouns: Option<Option<&str>>,
) -> MgmtPrincipal {
    let own = |v: Option<Option<&str>>| v.map(|x| x.map(str::to_string));
    m.email = own(email);
    m.title = own(title);
    m.pronouns = own(pronouns);
    m
}

#[test]
fn a8_fill_round_trips_only_fields_k2_does_not_own() {
    let a = uuid();
    // A principal: title/pronouns come back; the email stays K2's (here: none).
    let mut d = desired_person(&a, "ann", true, "r");
    let m = with_profile(
        member(
            ExternalId::K2Principal(a.clone()),
            MemberKind::Person,
            "ann",
            "ann",
        ),
        Some(Some("sew@x.io")),
        Some(Some("CTO")),
        Some(Some("she/her")),
    );
    assert!(chat::fill_provision_from_member(&mut d.request, &m));
    assert_eq!(d.request.profile.title.as_deref(), Some("CTO"));
    assert_eq!(d.request.profile.pronouns.as_deref(), Some("she/her"));
    assert_eq!(d.request.profile.email, None, "a principal's email is K2's");
    // An agent / the owner: the email is Sew's too.
    let w = uuid();
    let mut ag = desired_agent(&w, "bot", AgentKind::Assistant, Some(ExternalId::K2Owner));
    let mw = with_profile(
        member(
            ExternalId::K2Workspace(w.clone()),
            MemberKind::Agent,
            "bot",
            "bot",
        ),
        Some(Some("bot@x.io")),
        Some(None),
        None,
    );
    assert!(chat::fill_provision_from_member(&mut ag.request, &mw));
    assert_eq!(ag.request.profile.email.as_deref(), Some("bot@x.io"));
    assert_eq!(ag.request.profile.title, None, "Sew says none");
    assert_eq!(
        ag.request.profile.pronouns, None,
        "unknown (pre-SW14) stays absent"
    );
    // Nothing known → nothing changes.
    let mut d2 = desired_person(&a, "ann", false, "r");
    let bare = member(ExternalId::K2Principal(a.clone()), MemberKind::Person, "ann", "ann");
    assert!(!chat::fill_provision_from_member(&mut d2.request, &bare));
    // Sew 0.5.0 with K2's managed-fields policy: title/pronouns are the
    // member's own, never copied back and never sent.
    let mut d3 = desired_person(&a, "ann", false, "r");
    assert!(!chat::fill_provision_from_member_as(&mut d3.request, &m, true), "nothing K2 sends changed");
    assert_eq!((d3.request.profile.title.as_deref(), d3.request.profile.pronouns.as_deref()), (None, None));
    let mut d4 = desired_person(&a, "ann", false, "r");
    d4.request.profile.title = Some("stale".into());
    assert!(chat::fill_provision_from_member_as(&mut d4.request, &m, true), "a carried title is dropped");
    assert_eq!(d4.request.profile.title, None);
    let v = serde_json::to_value(&d4.request).unwrap();
    assert!(v["profile"].get("title").is_none() && v["profile"].get("pronouns").is_none(), "{v}");
}

#[test]
fn a8_reconcile_drift_uses_update_and_email_only_when_known() {
    let a = uuid();
    let pa = ExternalId::K2Principal(a.clone());
    let mut want = desired_person(&a, "ann", false, "r");
    want.request.profile.email = Some("ann@k2.io".into());
    // Sew's list carries a different email and a Sew-local title.
    let m = with_profile(
        member(pa.clone(), MemberKind::Person, "ann", "ann"),
        Some(Some("old@k2.io")),
        Some(Some("CTO")),
        None,
    );
    let (planned, s) = reconcile::plan(&[want.clone()], &[m], &HashSet::new(), &HashMap::new());
    assert_eq!(
        ops_for(&planned, &pa),
        [Op::Update],
        "profile drift → update, never provision"
    );
    assert_eq!(s.updated, 1);
    assert_eq!(
        planned[0].payload["profile"],
        serde_json::json!({"email": "ann@k2.io"}),
        "only the drifted field; the title is Sew's and never sent"
    );
    // K2 has no email, Sew has one → explicit clear.
    let mut no_mail = want.clone();
    no_mail.request.profile.email = None;
    let m2 = with_profile(
        member(pa.clone(), MemberKind::Person, "ann", "ann"),
        Some(Some("old@k2.io")),
        None,
        None,
    );
    let (planned, _) = reconcile::plan(&[no_mail], &[m2], &HashSet::new(), &HashMap::new());
    assert_eq!(
        planned[0].payload["profile"],
        serde_json::json!({"email": null})
    );
    // Email unknown (pre-SW14 list) → no drift, nothing planned.
    let m3 = member(pa.clone(), MemberKind::Person, "ann", "ann");
    let (planned, _) = reconcile::plan(&[want], &[m3], &HashSet::new(), &HashMap::new());
    assert!(planned.is_empty(), "{planned:?}");
}

#[test]
fn a8_reconcile_reprovision_round_trips_sew_fields() {
    let a = uuid();
    let pa = ExternalId::K2Principal(a.clone());
    // Guest flip needed (Sew: member, K2: guest).
    let want = desired_person(&a, "ann", true, "r");
    let m = with_profile(
        member(pa.clone(), MemberKind::Person, "ann", "ann"),
        Some(None),
        Some(Some("CTO")),
        Some(Some("she/her")),
    );
    let (planned, _) = reconcile::plan(
        &[want.clone()],
        &[m.clone()],
        &HashSet::new(),
        &HashMap::new(),
    );
    assert_eq!(ops_for(&planned, &pa), [Op::Provision]);
    let p = &planned[0].payload;
    assert_eq!(
        (
            p["guest"].as_bool(),
            p["profile"]["title"].as_str(),
            p["profile"]["pronouns"].as_str()
        ),
        (Some(true), Some("CTO"), Some("she/her"))
    );
    // A deactivated member is re-provisioned with its fields too.
    let mut gone = m;
    gone.deactivated = true;
    let (planned, _) = reconcile::plan(&[want], &[gone], &HashSet::new(), &HashMap::new());
    assert_eq!(planned[0].payload["profile"]["title"], "CTO");
}

#[test]
fn a8_filled_mark_is_set_once_and_keeps_the_key() {
    let _e = setup();
    enable_chat();
    let owner = rows_for("k2:owner").remove(0);
    assert_eq!(owner.filled_at, None);
    let mut body = payload(&owner);
    body["profile"]["title"] = serde_json::json!("Boss");
    record_filled(&owner.id, Some(&body)).unwrap();
    let after = row(&owner.id);
    assert!(after.filled_at.is_some());
    assert_eq!(payload(&after)["profile"]["title"], "Boss");
    assert_eq!(
        after.idem_key, owner.idem_key,
        "the key never changes on a fill"
    );
    // A second fill is a no-op: the persisted body is what every retry sends.
    let mut other = body.clone();
    other["profile"]["title"] = serde_json::json!("Changed");
    record_filled(&owner.id, Some(&other)).unwrap();
    assert_eq!(payload(&row(&owner.id))["profile"]["title"], "Boss");
}

#[test]
fn reconcile_keeps_the_last_change_and_a_short_history_past_no_op_passes() {
    let _e = setup();
    enable_chat();
    let pass = |at: i64, s: serde_json::Value| {
        skin::with_skin_conn(|conn| outbox::set_target_reconcile(conn, CHAT_KIND, Some(&s), None, at))
            .expect("record pass")
    };
    let zero = serde_json::to_value(reconcile::Summary::default()).unwrap();
    assert!(!outbox::reconcile_changed(&zero));
    let t = get_target(CHAT_KIND).unwrap().unwrap();
    assert!(t.last_change.is_none() && t.reconcile_history.is_empty());

    let mut changed = zero.clone();
    changed["provisioned"] = serde_json::json!(2);
    changed["orphansRemoved"] = serde_json::json!(1);
    assert!(outbox::reconcile_changed(&changed));
    pass(1_000, changed.clone());
    // Skips and ignores alone are not a change.
    let mut looked = zero.clone();
    looked["skippedBusy"] = serde_json::json!(3);
    looked["ignoredLocal"] = serde_json::json!(1);
    assert!(!outbox::reconcile_changed(&looked));
    pass(2_000, looked.clone());
    pass(3_000, zero.clone());

    let t = get_target(CHAT_KIND).unwrap().unwrap();
    assert_eq!(t.last_reconcile_at, Some(3_000), "the latest pass is still the latest");
    assert_eq!(t.last_reconcile.as_ref(), Some(&zero));
    assert_eq!(t.last_change_at, Some(1_000), "the last pass that changed something");
    assert_eq!(t.last_change.as_ref().unwrap()["provisioned"], 2);
    let ats: Vec<i64> = t.reconcile_history.iter().map(|h| h["at"].as_i64().unwrap()).collect();
    assert_eq!(ats, vec![1_000, 2_000, 3_000], "oldest first, each pass stamped");
    assert_eq!(t.reconcile_history[1]["skippedBusy"], 3);

    // The history is bounded; the last change outlives it.
    for i in 0..(outbox::RECONCILE_HISTORY as i64 + 5) {
        pass(10_000 + i, zero.clone());
    }
    let t = get_target(CHAT_KIND).unwrap().unwrap();
    assert_eq!(t.reconcile_history.len(), outbox::RECONCILE_HISTORY);
    assert_eq!(
        t.reconcile_history.last().unwrap()["at"].as_i64(),
        Some(10_000 + outbox::RECONCILE_HISTORY as i64 + 4)
    );
    assert_eq!(t.last_change_at, Some(1_000));
    // The wire shape the CLI reads.
    let v = serde_json::to_value(&t).unwrap();
    assert_eq!(v["lastChangeAt"], 1_000);
    assert!(v["reconcileHistory"].is_array());
}
