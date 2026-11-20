//! `/cli/sew/owner-sessions*` against S27's outbox (a recording sink, and
//! the real `tool_outbox` in a temp `skin.db`). Fail loudly: every
//! expectation is an assert on the exact code, status or op.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use k2_core::connect_users::Role;
use k2_core::sew_revoke::{self, SewSessionRevoke};

use super::routes::{self, now};
use super::routes_approvals::test_seams as approval_seams;
use crate::chat_pass::handoff::END_CONFIRM_ACTION;
use crate::chat_pass::owner_sessions;
use crate::compute_routes::Who;

const SID_A: &str = "own_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SID_B: &str = "own_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const LIST: &str = "/cli/sew/owner-sessions";
const REVOKE: &str = "/cli/sew/owner-sessions/revoke";

struct Env {
    _db: k2_core::db::TestDbGuard,
    _home: k2_core::test_env::TempHome,
    /// The ops the outbox was handed (`None`: the real outbox).
    seen: Option<Arc<Mutex<Vec<SewSessionRevoke>>>>,
}

impl Drop for Env {
    fn drop(&mut self) {
        approval_seams::set_vault(None);
        sew_revoke::clear_sink();
    }
}

impl Env {
    fn seen(&self) -> Vec<SewSessionRevoke> {
        self.seen.as_ref().expect("a sink env").lock().unwrap().clone()
    }
}

/// A temp home and DB, a test vault, and (with `sink`) a recording outbox.
fn env(sink: bool) -> Env {
    let home = k2_core::test_env::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    approval_seams::set_vault(Some(crate::compute_admin::test_seams::vault_handle()));
    let seen = sink.then(|| {
        let seen: Arc<Mutex<Vec<SewSessionRevoke>>> = Arc::default();
        let s = seen.clone();
        sew_revoke::set_sink(Arc::new(move |_c: &rusqlite::Connection, op: &SewSessionRevoke| {
            s.lock().unwrap().push(op.clone());
            Ok(())
        }));
        seen
    });
    if !sink {
        sew_revoke::clear_sink();
    }
    Env { _db: db, _home: home, seen }
}

fn status_code(s: &str) -> u16 {
    s.split_whitespace().next().unwrap().parse().unwrap()
}

fn post(who: &Who, body: Value) -> (u16, Value) {
    let r = routes::handle(REVOKE, who.clone(), body.to_string().as_bytes(), &HashMap::new(), true);
    (status_code(r.status), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

fn list(who: &Who) -> (u16, Value) {
    let r = routes::handle(LIST, who.clone(), b"", &HashMap::new(), false);
    (status_code(r.status), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{e}: {}", r.body)))
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error code in {v}"))
}

fn mint(secret: &str, stamp: &str) -> String {
    crate::compute_admin::test_seams::vault_handle().confirm.mint(END_CONFIRM_ACTION, Some(secret), stamp, "vault-password")
}

fn record(sid: &str, ago: i64) {
    let t = now() - ago;
    owner_sessions::record_redeemed(sid, "owner-token", t - 5, t).expect("record");
}

fn agent() -> Who {
    Who::Agent { project_id: "aaaaaaaa-0000-4000-8000-000000000001".into(), session_id: "sess-a".into() }
}

fn shell() -> Who {
    Who::Shell { project_id: "aaaaaaaa-0000-4000-8000-000000000001".into(), session_id: "sess-s".into() }
}

fn admin() -> Who {
    Who::Login { username: "ada".into(), role: Role::Admin }
}

fn session<'a>(v: &'a Value, sid: &str) -> &'a Value {
    v["sessions"].as_array().expect("sessions").iter().find(|s| s["sid"] == sid).unwrap_or_else(|| panic!("{sid} not in {v}"))
}

#[test]
fn the_list_shows_what_k2_recorded_and_hides_entries_30_days_old() {
    let _e = env(true);
    record(SID_A, 10);
    record(SID_B, owner_sessions::KEEP_SECS + 1);
    let (s, v) = list(&Who::Owner);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["sessions"].as_array().unwrap().len(), 1, "{v}");
    let a = session(&v, SID_A);
    let t = now() - 10;
    assert_eq!(
        *a,
        json!({"sid": SID_A, "mintedBy": "owner-token", "mintedAt": t - 5, "redeemedAt": t, "endQueued": false, "delivered": false})
    );
    assert!(v["takesEffect"].as_str().unwrap().contains("as soon as Sew receives it"), "{v}");
    let (s, v) = list(&admin());
    assert_eq!(s, 200, "an admin sees them: {v}");
}

#[test]
fn ending_one_session_queues_its_k2_sid_after_a_confirm_bound_to_it() {
    let e = env(true);
    record(SID_A, 10);
    record(SID_B, 20);
    // No token: the daemon names the action and the sid; nothing queued.
    let (s, v) = post(&Who::Owner, json!({"sid": SID_A}));
    assert_eq!((s, code(&v)), (403, "confirm_required"), "{v}");
    assert_eq!(v["error"]["details"]["action"], "chat-owner-sessions-end");
    assert_eq!(v["error"]["details"]["secretId"], SID_A);
    assert!(e.seen().is_empty());
    // A confirm for another sid, or for all, doesn't end this one.
    let (s, v) = post(&Who::Owner, json!({"sid": SID_A, "confirmToken": mint(SID_B, "owner")}));
    assert_eq!((s, code(&v)), (403, "confirm_required"), "{v}");
    let (s, v) = post(&Who::Owner, json!({"sid": SID_A, "confirmToken": mint("all", "owner")}));
    assert_eq!((s, code(&v)), (403, "confirm_required"), "{v}");
    assert!(e.seen().is_empty());
    let (s, v) = post(&Who::Owner, json!({"sid": SID_A, "confirmToken": mint(SID_A, "owner")}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(e.seen(), vec![SewSessionRevoke::K2Sid(SID_A.into())]);
    assert_eq!(v["queued"], format!("k2_sid={SID_A}"));
    assert_eq!(v["ended"], json!([SID_A]));
    assert_eq!(v["confirmedBy"], "vault-password");
    let (_, v) = list(&Who::Owner);
    let a = session(&v, SID_A);
    assert_eq!(a["endQueued"], true, "{v}");
    assert_eq!(a["end"]["via"], "sid");
    assert_eq!(a["end"]["by"], "owner");
    assert_eq!(session(&v, SID_B)["endQueued"], false, "{v}");
    // The administration log names it, never a token.
    let rows = {
        let h = k2_core::db::shared();
        let c = h.lock();
        k2_core::sew_chat::store::audit_recent(&c, 10).expect("audit")
    };
    let row = rows.iter().find(|r| r.op == "owner_sessions.end").expect("audit row");
    assert_eq!((row.actor.as_str(), row.target.as_deref()), ("owner", Some(SID_A)));
}

#[test]
fn ending_all_queues_external_id_k2_owner_and_marks_every_unended_session() {
    let e = env(true);
    record(SID_A, 10);
    record(SID_B, 20);
    let (s, v) = post(&admin(), json!({"sid": SID_A, "confirmToken": mint(SID_A, "connect:ada")}));
    assert_eq!(s, 200, "an admin ends one: {v}");
    // A sid's confirm is not --all's.
    let (s, v) = post(&admin(), json!({"all": true, "confirmToken": mint(SID_B, "connect:ada")}));
    assert_eq!((s, code(&v)), (403, "confirm_required"), "{v}");
    assert_eq!(v["error"]["details"]["secretId"], "all");
    // Another person's confirm is not this caller's.
    let (s, v) = post(&admin(), json!({"all": true, "confirmToken": mint("all", "owner")}));
    assert_eq!((s, code(&v)), (403, "confirm_required"), "{v}");
    let (s, v) = post(&admin(), json!({"all": true, "confirmToken": mint("all", "connect:ada")}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(e.seen(), vec![SewSessionRevoke::K2Sid(SID_A.into()), SewSessionRevoke::ExternalId("k2:owner".into())]);
    assert_eq!(v["queued"], "external_id=k2:owner");
    assert_eq!(v["ended"], json!([SID_B]), "an earlier end stays as it was");
    let (_, v) = list(&Who::Owner);
    assert_eq!(session(&v, SID_B)["end"]["via"], "all", "{v}");
    assert_eq!(session(&v, SID_B)["end"]["by"], "login:ada", "{v}");
    assert_eq!(session(&v, SID_A)["end"]["via"], "sid", "{v}");
    // With nothing recorded, --all still queues (an unlisted session ends too).
    std::fs::remove_file(owner_sessions::path()).expect("forget the record");
    let (s, v) = post(&Who::Owner, json!({"all": true, "confirmToken": mint("all", "owner")}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["ended"], json!([]));
    assert_eq!(e.seen().last(), Some(&SewSessionRevoke::ExternalId("k2:owner".into())));
}

#[test]
fn agents_shell_tabs_app_passes_and_members_are_refused() {
    let e = env(true);
    record(SID_A, 10);
    for who in [agent(), shell()] {
        let (s, v) = list(&who);
        assert_eq!((s, code(&v)), (403, "person_only"), "{who:?}: {v}");
        // Even holding a valid confirm token, an agent ends nothing.
        let (s, v) = post(&who, json!({"sid": SID_A, "confirmToken": mint(SID_A, "owner")}));
        assert_eq!((s, code(&v)), (403, "person_only"), "{who:?}: {v}");
        let (s, v) = post(&who, json!({"all": true}));
        assert_eq!((s, code(&v)), (403, "person_only"), "{who:?}: {v}");
    }
    let member = Who::Login { username: "mo".into(), role: Role::Member };
    let (s, v) = post(&member, json!({"all": true}));
    assert_eq!((s, code(&v)), (403, "owner_only"), "{v}");
    let (s, v) = list(&member);
    assert_eq!((s, code(&v)), (403, "owner_only"), "{v}");
    let (s, v) = post(&Who::Wrong, json!({"all": true}));
    assert_eq!((s, code(&v)), (403, "wrong_credential"), "{v}");
    let (s, v) = list(&Who::Invalid);
    assert_eq!(s, 401, "{v}");
    assert!(e.seen().is_empty(), "nothing queued");
    assert!(!crate::session_token::is_agent_verb(LIST) && !crate::session_token::is_agent_verb(REVOKE));
}

#[test]
fn bodies_are_checked_and_an_unknown_sid_spends_no_confirm() {
    let e = env(true);
    record(SID_A, 10);
    for body in [json!({}), json!({"sid": SID_A, "all": true}), json!({"sid": "own_short"}), json!({"sid": SID_A, "extra": 1})] {
        let (s, v) = post(&Who::Owner, body.clone());
        assert_eq!(s, 400, "{body}: {v}");
    }
    let tok = mint(SID_B, "owner");
    let (s, v) = post(&Who::Owner, json!({"sid": SID_B, "confirmToken": tok}));
    assert_eq!((s, code(&v)), (404, "not_found"), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("end --all"), "{v}");
    // The token was not spent on the 404: record it and it still works.
    record(SID_B, 5);
    let (s, v) = post(&Who::Owner, json!({"sid": SID_B, "confirmToken": tok}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(e.seen(), vec![SewSessionRevoke::K2Sid(SID_B.into())]);
    let r = routes::handle(REVOKE, Who::Owner, b"{}", &HashMap::new(), false);
    assert_eq!(status_code(r.status), 405);
}

#[test]
fn the_real_outbox_gets_the_row_and_the_list_says_when_it_is_delivered() {
    let _e = env(false);
    k2_core::tool_hook::allow_target_off_linux_for_tests(true);
    k2_core::tool_hook::set_target("chat", true, Some("/tmp/k2-owner-sessions-sew.sock"), Some("/tmp/k2-owner-sessions-sew.token"), "owner-token")
        .expect("chat target");
    record(SID_A, 10);
    let (s, v) = post(&Who::Owner, json!({"sid": SID_A, "confirmToken": mint(SID_A, "owner")}));
    assert_eq!(s, 200, "{v}");
    let rows = k2_core::tool_hook::log(50, Some(&format!("revoke:k2_sid:{SID_A}"))).expect("outbox");
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!((row.op.as_str(), row.state.as_str()), ("sew_session_revoke", "pending"));
    let (_, v) = list(&Who::Owner);
    let a = session(&v, SID_A);
    assert_eq!((a["endQueued"].clone(), a["delivered"].clone(), a["end"]["outbox"].clone()), (json!(true), json!(false), json!("pending")), "{v}");
    k2_core::tool_hook::record_done(&row.id, 200, r#"{"revoked":1}"#).expect("deliver");
    let (_, v) = list(&Who::Owner);
    let a = session(&v, SID_A);
    assert_eq!((a["delivered"].clone(), a["end"]["outbox"].clone()), (json!(true), json!("done")), "{v}");
    // --all rides its own subject.
    let (s, v) = post(&Who::Owner, json!({"all": true, "confirmToken": mint("all", "owner")}));
    assert_eq!(s, 200, "{v}");
    let rows = k2_core::tool_hook::log(50, Some("revoke:k2:owner")).expect("outbox");
    assert_eq!(rows.len(), 1, "{rows:?}");
}

#[test]
fn without_chat_set_up_nothing_is_queued_or_marked() {
    let _e = env(false);
    record(SID_A, 10);
    let (s, v) = post(&Who::Owner, json!({"sid": SID_A, "confirmToken": mint(SID_A, "owner")}));
    assert_eq!((s, code(&v)), (409, "chat_not_set_up"), "{v}");
    let (_, v) = list(&Who::Owner);
    assert_eq!(session(&v, SID_A)["endQueued"], false, "{v}");
}
