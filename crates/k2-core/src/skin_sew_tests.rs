//! Sew S28 (K7/K8) core tests: the `chat` grant check, chat-only login,
//! `sew_session_revoke` ops on every session-revoke path (same Immediate
//! transaction), and Connect accounts' stable never-reused ids.
//!
//! Every test runs under `with_temp_home` (the global env lock), so the
//! process-wide recording sink below is only ever read by one test at a
//! time.

use std::sync::{Arc, Mutex, Once};

use super::*;
use crate::connect_users;
use crate::sew_revoke::{self, SewSessionRevoke};
use crate::tunnel::test_support::with_temp_home;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    op: SewSessionRevoke,
    in_tx: bool,
}

static SEEN: Mutex<Vec<Seen>> = Mutex::new(Vec::new());
/// Ops whose label contains one of these fail in the sink.
static FAIL_ON: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn recorder() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        sew_revoke::set_sink(Arc::new(|conn: &rusqlite::Connection, op: &SewSessionRevoke| {
            if FAIL_ON.lock().unwrap().iter().any(|f| op.label().contains(f.as_str())) {
                return Err("outbox write failed (test)".into());
            }
            SEEN.lock().unwrap().push(Seen { op: op.clone(), in_tx: !conn.is_autocommit() });
            Ok(())
        }));
    });
    SEEN.lock().unwrap().clear();
    FAIL_ON.lock().unwrap().clear();
}

fn seen() -> Vec<Seen> {
    SEEN.lock().unwrap().clone()
}

fn guest(name: &str, chat: bool) -> SkinPrincipal {
    let p = add_principal(name).expect("add principal");
    set_principal_password(name, Some("s3cret-horse-battery")).expect("password");
    if chat {
        insert_chat_grant_for_tests(&p.id).expect("chat grant");
    }
    p
}

fn host_grant_id(principal_id: &str) -> String {
    list_grants()
        .expect("grants")
        .into_iter()
        .find(|g| g.subject_id == principal_id && g.kind == "app" && g.target_id == HOST_APP_ID)
        .expect("host grant")
        .id
}

#[test]
fn chat_grant_is_read_live_and_must_be_enabled_on_sew() {
    with_temp_home(|| {
        recorder();
        let ada = guest("ada", false);
        assert!(!principal_has_chat_grant(&ada.id).expect("lookup"));
        insert_chat_grant_for_tests(&ada.id).expect("grant");
        assert!(principal_has_chat_grant(&ada.id).expect("lookup"));
        delete_chat_grant_for_tests(&ada.id).expect("delete");
        assert!(!principal_has_chat_grant(&ada.id).expect("lookup"));
    });
}

#[test]
fn revoke_token_queues_its_skn_sid_inside_the_transaction() {
    with_temp_home(|| {
        recorder();
        guest("ada", true);
        let (meta, raw) = create_session_token("ada").expect("login");
        assert!(resolve_skin_token(&raw).is_some());
        assert!(revoke_token(&meta.id).expect("revoke"));
        assert_eq!(seen(), vec![Seen { op: SewSessionRevoke::K2Sid(format!("skn_{}", meta.id)), in_tx: true }]);
        // A second revoke changes nothing and queues nothing.
        assert!(!revoke_token(&meta.id).expect("revoke again"));
        assert_eq!(seen().len(), 1);
        assert!(resolve_skin_token(&raw).is_none());
    });
}

#[test]
fn no_chat_grant_means_no_revoke_op() {
    with_temp_home(|| {
        recorder();
        guest("bob", false);
        let (meta, _raw) = create_session_token("bob").expect("login");
        assert!(revoke_token(&meta.id).expect("revoke"));
        set_principal_password("bob", Some("another-horse-battery")).expect("pw");
        assert!(remove_principal("bob").expect("remove"));
        assert_eq!(seen(), vec![]);
    });
}

#[test]
fn password_change_and_removal_revoke_by_external_id() {
    with_temp_home(|| {
        recorder();
        let ada = guest("ada", true);
        let (_m1, raw1) = create_session_token("ada").expect("login 1");
        let (_m2, _raw2) = create_session_token("ada").expect("login 2");
        set_principal_password("ada", Some("a-new-horse-battery")).expect("pw change");
        let ext = SewSessionRevoke::ExternalId(format!("k2:principal:{}", ada.id));
        assert_eq!(seen(), vec![Seen { op: ext.clone(), in_tx: true }], "one op for both sessions");
        assert!(resolve_skin_token(&raw1).is_none());
        // No live session left: a second password write queues nothing.
        set_principal_password("ada", Some("third-horse-battery")).expect("pw again");
        assert_eq!(seen().len(), 1);
        // Removal: the person's Sew sessions go with them.
        let _ = create_session_token("ada").expect("login 3");
        assert!(remove_principal("ada").expect("remove"));
        assert_eq!(seen(), vec![Seen { op: ext.clone(), in_tx: true }, Seen { op: ext, in_tx: true }]);
    });
}

#[test]
fn app_grant_off_and_service_removal_revoke_those_sessions_only() {
    with_temp_home(|| {
        recorder();
        let ada = guest("ada", true);
        let (host_meta, _) = create_session_token("ada").expect("host login");
        // An App session (S24) bound to service `svc1`.
        create_grant(&SkinGrantWrite {
            subject_kind: "principal".into(),
            subject_id: ada.id.clone(),
            kind: "app".into(),
            target_id: "svc1".into(),
            role: None,
            scope: Some("[]".into()),
            created_by: None,
        })
        .expect("app grant");
        let app = create_app_session_token("ada", "svc1").expect("app login");
        assert_eq!(revoke_sessions_for_app("svc1").expect("publish rm"), 1);
        assert_eq!(seen(), vec![Seen { op: SewSessionRevoke::K2Sid(format!("skn_{}", app.meta.id)), in_tx: true }]);
        // Switching the host grant off ends the host session (per-App).
        set_grant_enabled(&host_grant_id(&ada.id), false).expect("disable host grant");
        assert_eq!(
            seen()[1..].to_vec(),
            vec![Seen { op: SewSessionRevoke::K2Sid(format!("skn_{}", host_meta.id)), in_tx: true }]
        );
    });
}

#[test]
fn outbox_failure_rolls_the_revoke_back() {
    with_temp_home(|| {
        recorder();
        guest("ada", true);
        let (meta, raw) = create_session_token("ada").expect("login");
        FAIL_ON.lock().unwrap().push(meta.id.clone());
        let err = revoke_token(&meta.id).expect_err("the outbox write fails");
        assert!(err.contains("outbox write failed"), "{err}");
        assert!(resolve_skin_token(&raw).is_some(), "revoke rolled back with its op");
        FAIL_ON.lock().unwrap().clear();
        assert!(revoke_token(&meta.id).expect("revoke"));
        assert!(resolve_skin_token(&raw).is_none());
        assert_eq!(seen().len(), 1);
    });
}

#[test]
fn chat_only_login_is_thread_dark_and_dies_with_the_grant() {
    with_temp_home(|| {
        recorder();
        let ada = guest("ada", false);
        assert!(delete_grant_by_key("principal", &ada.id, "app", HOST_APP_ID).expect("drop host grant"));
        // Neither grant: the same denial as a bad password.
        assert_eq!(create_session_token("ada").expect_err("no grant"), HOST_LOGIN_DENIED);
        // Chat only: a box login with an empty room policy.
        insert_chat_grant_for_tests(&ada.id).expect("chat grant");
        let (meta, raw) = create_session_token("ada").expect("chat-only login");
        assert!(meta.rooms.is_empty(), "{meta:?}");
        let pass = resolve_skin_token(&raw).expect("chat-only session resolves");
        assert!(pass.session);
        assert!(pass.room_policy.is_empty(), "Thread dark: {:?}", pass.room_policy);
        assert!(!pass.has_room("room-1"));
        assert_eq!(pass.app, HOST_APP_ID);
        // An App login still needs that App's own grant.
        assert_eq!(create_app_session_token("ada", "svc1").expect_err("no app grant").as_str(), HOST_LOGIN_DENIED);
        // The chat grant goes → the chat-only session stops resolving.
        delete_chat_grant_for_tests(&ada.id).expect("drop chat grant");
        assert!(resolve_skin_token(&raw).is_none());
    });
}

#[test]
fn host_session_with_rooms_does_not_survive_on_a_chat_grant() {
    with_temp_home(|| {
        recorder();
        let ada = guest("ada", true);
        let role = create_role("readers", &thread_only_room_policy(&["room-1".to_string()])).expect("role");
        assign_role("ada", &role.id).expect("assign");
        let (_meta, raw) = create_session_token("ada").expect("host login");
        assert!(!resolve_skin_token(&raw).expect("live").room_policy.is_empty());
        // Drop the host grant behind the revoke writers' back: the backstop
        // must not keep a roomful session alive on the chat grant.
        with_conn(|conn| {
            conn.execute("DELETE FROM grants WHERE subject_id = ?1 AND kind = 'app'", params![ada.id])
                .map_err(|e| e.to_string())
        })
        .expect("raw delete");
        assert!(resolve_skin_token(&raw).is_none());
    });
}

#[test]
fn connect_ids_are_stable_backfilled_and_never_reused() {
    with_temp_home(|| {
        recorder();
        // A store written before ids existed.
        let legacy = serde_json::json!({
            "users": [
                {"username": "old1", "password_hash": "$argon2id$x", "created_at": "2026-01-01T00:00:00Z", "role": "owner"},
                {"username": "old2", "password_hash": "$argon2id$x", "created_at": "2026-01-01T00:00:00Z"}
            ]
        });
        std::fs::write(connect_users::store_path(), legacy.to_string()).expect("write legacy store");
        let a1 = connect_users::user_uuid("old1").expect("uuid").expect("exists");
        let a2 = connect_users::user_uuid("old2").expect("uuid").expect("exists");
        assert!(uuid::Uuid::parse_str(&a1).is_ok() && uuid::Uuid::parse_str(&a2).is_ok());
        assert_ne!(a1, a2);
        // Backfilled on disk: stable across reads, boot and later writes.
        let raw = std::fs::read_to_string(connect_users::store_path()).expect("store");
        assert!(raw.contains(&a1) && raw.contains(&a2), "{raw}");
        assert_eq!(connect_users::backfill_user_ids().expect("boot"), 0);
        connect_users::set_role("old2", connect_users::Role::Admin).expect("write");
        assert_eq!(connect_users::user_uuid("old1").expect("uuid"), Some(a1.clone()));
        assert_eq!(connect_users::user_uuid("old2").expect("uuid"), Some(a2.clone()));
        // Boot backfill on a fresh legacy row.
        connect_users::add_user("fresh", "password-for-tests-1").expect("add");
        let f1 = connect_users::user_uuid("fresh").expect("uuid").expect("exists");
        assert!(!f1.is_empty());
        // Remove and add the same username: a new id, never the old one.
        connect_users::remove_user("old1").expect("remove");
        assert_eq!(connect_users::user_uuid("old1").expect("uuid"), None);
        connect_users::add_user("old1", "password-for-tests-1").expect("re-add");
        let again = connect_users::user_uuid("old1").expect("uuid").expect("exists");
        assert_ne!(again, a1, "a removed person's id is never reused");
        let raw = std::fs::read_to_string(connect_users::store_path()).expect("store");
        assert!(raw.contains("retired_ids") && raw.contains(&a1), "retired id kept: {raw}");
        // A hand-edited row that reuses a retired id gets a fresh one.
        let mut v: serde_json::Value = serde_json::from_str(&raw).expect("json");
        v["users"].as_array_mut().expect("users").iter_mut().find(|u| u["username"] == "old2").expect("old2")["id"] =
            serde_json::json!(a1.clone());
        std::fs::write(connect_users::store_path(), v.to_string()).expect("write");
        let healed = connect_users::user_uuid("old2").expect("uuid").expect("exists");
        assert_ne!(healed, a1);
    });
}

#[test]
fn connect_logout_and_revoke_all_queue_cu_sids() {
    with_temp_home(|| {
        recorder();
        connect_users::add_user("carol", "password-for-tests-1").expect("add");
        let t1 = connect_users::create_session("carol");
        let t2 = connect_users::create_session("carol");
        let sid1 = connect_users::session_sid(&t1);
        assert!(sid1.starts_with("cu_") && sid1.len() == 35, "{sid1}");
        assert!(!sid1.contains(&t1[..16]), "the sid is a hash, not the token");
        assert_eq!(connect_users::logout_session(&t1).as_deref(), Some("carol"));
        assert_eq!(seen(), vec![Seen { op: SewSessionRevoke::K2Sid(sid1), in_tx: true }]);
        // Unknown token: nothing.
        assert_eq!(connect_users::logout_session(&t1), None);
        assert_eq!(seen().len(), 1);
        // Disable = log out everywhere: one op per ended session.
        connect_users::set_disabled("carol", true).expect("disable");
        assert_eq!(seen()[1..].to_vec(), vec![Seen { op: SewSessionRevoke::K2Sid(connect_users::session_sid(&t2)), in_tx: true }]);
    });
}
