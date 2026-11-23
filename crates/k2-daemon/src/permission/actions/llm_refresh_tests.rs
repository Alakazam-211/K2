//! A field report: the server default's access token expired, the
//! list still said `signed_in`, and an agent's `k2 llm tokens refresh` was a
//! dead end (`owner_only`).
//!
//! - the list serves `access`: fine / expired (with who renews it) / signed
//!   out, to people and agents alike, never a token value;
//! - an agent's refresh is still refused, with a hint that says when nothing
//!   is needed, and (cards on) the `llm.tokens-refresh` card's line;
//! - the card end to end: a Member can't answer, an Admin's Allow runs the
//!   route's own recheck as the Admin, and the agent's turn names each
//!   token's state.
//!
//! Test DB, temp HOME, fake credentials with a marker; no network, no real
//! agent is woken.

use std::collections::HashMap;

use k2_core::connect_users::Role;
use k2_core::llm_accounts::{self as wallet_core, state, store, wallet, MetaUpdate, Tool};
use k2_core::permission::catalog::{Direction, Floor, Grant, ParamKind, Proof, Undo};
use serde_json::{json, Value};

use super::super::held::tests::{answer_as, ask, ask_ok, ask_refused, below_floor, card_of, last_turn, login, seed, view};
use super::*;
use crate::llm_accounts_routes::{self as routes, LIST};

const MARKER: &str = "K2TEST_SECRET_MARKER";

fn env() -> crate::test_support::TempHome {
    let home = crate::test_support::TempHome::new();
    k2_core::db::init_for_tests();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("DELETE FROM llm_active", []).unwrap();
    conn.execute("DELETE FROM llm_accounts", []).unwrap();
    home
}

fn cred(tag: &str, expires_ms: i64) -> String {
    format!(r#"{{"claudeAiOauth":{{"accessToken":"{MARKER}-at-{tag}","refreshToken":"{MARKER}-rt-{tag}","expiresAt":{expires_ms}}}}}"#)
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn with_conn<T>(f: impl FnOnce(&rusqlite::Connection) -> T) -> T {
    let db = k2_core::db::shared();
    let conn = db.lock();
    f(&conn)
}

/// An idle Claude token whose slot holds `cred`, with `meta` on its row.
fn idle(label: &str, cred_bytes: &str, meta: MetaUpdate) -> String {
    let e = with_conn(|conn| wallet_core::create(conn, Tool::Claude, label, None)).expect("create");
    let dir = store::slot_dir(Tool::Claude, &e.id);
    store::ensure_private_dir(&dir).unwrap();
    std::fs::write(dir.join(".credentials.json"), cred_bytes).unwrap();
    with_conn(|conn| wallet_core::update_meta(conn, &e.id, &meta)).expect("meta");
    e.id
}

fn agent() -> crate::sidecar_routes::Caller {
    crate::sidecar_routes::Caller::Agent(crate::sidecar_routes::AgentCaller { project_id: "ws-test".into(), map_key: None, args: vec![], sandboxed: false })
}

fn call(path: &str, caller: crate::sidecar_routes::Caller, body: &str) -> (String, Value) {
    let r = routes::handle(path, caller, body.as_bytes(), &HashMap::new(), "test");
    assert!(!r.body.contains(MARKER), "{path}: a token value in the answer: {}", r.body);
    (r.status.to_string(), serde_json::from_str(&r.body).expect("json body"))
}

fn account<'a>(list: &'a Value, id: &str) -> &'a Value {
    list["tools"]
        .as_array()
        .and_then(|t| t.iter().flat_map(|t| t["accounts"].as_array().into_iter().flatten()).find(|a| a["id"] == id))
        .unwrap_or_else(|| panic!("{id} not listed: {list}"))
}

#[test]
fn the_card_is_registered_at_admin_with_the_routes_member_floor_under_it() {
    let _db = k2_core::db::scoped_for_test();
    let r = super::super::find(ACTION).expect("registered");
    k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((r.spec.floor, r.spec.proof, r.spec.grant, r.spec.direction), (Floor::Admin, Proof::Click, Grant::OneTime, Direction::Loosen));
    assert!(matches!(r.spec.undo, Undo::None));
    let params: Vec<(&str, ParamKind, bool)> = r.spec.params.iter().map(|p| (p.key, p.kind, p.required)).collect();
    assert_eq!(params, [("token", ParamKind::Text, false)]);
    let floor = crate::routes::route_policy::lookup(ROUTE).and_then(|x| x.post).expect("the route has a POST floor");
    assert!(floor.admits(Role::Admin) && floor.admits(Role::Owner), "{ROUTE}: the card's answerers may use the route");
    assert!(crate::permission::walls::V1_ACTIONS.contains(&ACTION));
}

#[test]
fn the_list_says_fine_access_expired_and_signed_out_to_people_and_agents() {
    let _db = k2_core::db::scoped_for_test();
    let home = env();
    // The server default: its access token expired an hour ago, its sign-in
    // is good (the report).
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    std::fs::write(home.path().join(".claude/.credentials.json"), cred("def", (now() - 3600) * 1000)).unwrap();
    let def = with_conn(|conn| wallet::import_live(conn, Tool::Claude, None)).unwrap().expect("default").id;
    let far = 4_102_444_800_000_i64;
    let fine = idle("fine", &cred("fine", far), MetaUpdate { state: Some(state::SIGNED_IN.into()), expires_at: Some(Some(far / 1000)), ..Default::default() });
    let stale = idle(
        "stale",
        &cred("stale", (now() - 60) * 1000),
        MetaUpdate { state: Some(state::SIGNED_IN.into()), expires_at: Some(Some(now() - 60)), ..Default::default() },
    );
    let ended = idle(
        "ended",
        r#"{"claudeAiOauth":{"accessToken":"","refreshToken":"","expiresAt":0,"loginEnded":"time_limit"}}"#,
        MetaUpdate {
            state: Some(state::NEEDS_LOGIN.into()),
            detail: Some(Some("Claude's sign-in reached its time limit; sign in again.".into())),
            ..Default::default()
        },
    );
    for caller in [crate::sidecar_routes::Caller::Owner, agent()] {
        let (s, list) = call(LIST, caller, "");
        assert_eq!(s, "200 OK", "{list}");
        let d = account(&list, &def);
        assert_eq!((&d["active"], &d["state"], &d["access"]), (&json!(true), &json!("signed_in"), &json!("expired")), "{d}");
        let note = d["accessNote"].as_str().expect("a note");
        assert!(note.contains("Claude renews it by itself the next time it runs on this token") && note.contains("Nothing to do"), "{note}");
        let f = account(&list, &fine);
        assert_eq!((&f["access"], &f["accessNote"]), (&json!("fine"), &Value::Null), "{f}");
        let x = account(&list, &stale);
        assert_eq!(x["access"], "expired", "{x}");
        assert!(x["accessNote"].as_str().is_some_and(|n| n.contains("the next keep-warm pass")), "an idle token is K2's to renew: {x}");
        let e = account(&list, &ended);
        assert_eq!((&e["state"], &e["access"], &e["accessNote"]), (&json!("needs_login"), &json!("signed_out"), &Value::Null), "{e}");
    }
}

#[test]
fn an_agents_refresh_is_refused_with_what_to_do_instead() {
    let _db = k2_core::db::scoped_for_test();
    let _home = env();
    let (s, v) = call(ROUTE, agent(), r#"{"id":"acc_nope"}"#);
    assert_eq!((s.as_str(), &v["error"]["code"]), ("403 Forbidden", &json!("owner_only")), "{v}");
    let hint = v["error"]["hint"].as_str().expect("hint");
    assert_eq!(hint, routes::REFRESH_AGENT_HINT);
    assert!(hint.contains("run the Ask with: line below"), "{hint}");
    assert!(hint.contains("llm tokens list says access_expired") && hint.contains("renews itself the next time its CLI runs"), "{hint}");
    assert!(hint.contains("needs_login needs a person to sign it in again"), "{hint}");
    // People still recheck it themselves.
    let (s, v) = call(ROUTE, crate::sidecar_routes::Caller::Owner, "{}");
    assert_eq!(s, "200 OK", "{v}");
}

#[test]
fn an_agent_asks_and_an_admins_allow_rechecks_the_token_and_says_its_state() {
    let _db = k2_core::db::scoped_for_test();
    let _home = env();
    let me = seed("llmr");
    // An idle token whose access token expired; its row still says fine.
    let id = idle(
        "work",
        &cred("work", (now() - 120) * 1000),
        MetaUpdate { state: Some(state::SIGNED_IN.into()), expires_at: Some(Some(4_102_444_800)), ..Default::default() },
    );

    let asked = ask_ok(&me, ACTION, json!({ "token": id }));
    let card = card_of(&asked);
    let vw = view(&me, &card);
    assert_eq!(vw["whatChanges"], "Recheck the Claude token \"work\" on this server.", "{vw}");
    let effects = vw["consequences"].as_str().expect("consequences");
    assert!(effects.contains("Nothing is signed in, switched or removed, no token is shown"), "{effects}");
    assert!(effects.contains("the server default is never refreshed"), "{effects}");

    // A Member is below the card's floor; nothing is rechecked.
    let member = login("llmrm", Role::Member);
    below_floor(answer_as(&member, &me, &card, json!({ "answer": "Allow" })), "admin");
    let row = with_conn(|conn| wallet_core::get(conn, &id)).unwrap().unwrap();
    assert_eq!(row.expires_at, Some(4_102_444_800), "untouched before the Allow");

    // An Admin taps Allow: the route's own recheck, as the Admin, once.
    let admin = login("llmra", Role::Admin);
    let (s, a) = answer_as(&admin, &me, &card, json!({ "answer": "Allow" }));
    assert_eq!(s, "200 OK", "{a}");
    assert_eq!(a["action"]["result"]["outcome"], "approved", "{a}");
    let row = with_conn(|conn| wallet_core::get(conn, &id)).unwrap().unwrap();
    assert!(row.expires_at.is_some_and(|x| x <= now()), "the recheck recorded the slot's real expiry: {row:?}");
    let turn = last_turn(&me);
    assert!(turn.contains("Rechecked. Claude \"work\": signed in. Its access token expired on"), "{turn}");
    assert!(turn.contains("the next keep-warm pass"), "{turn}");
    assert!(!turn.contains(MARKER), "{turn}");

    // Every token, when the refused refresh named none.
    let all = ask_ok(&me, ACTION, json!({}));
    assert_eq!(view(&me, &card_of(&all))["whatChanges"], "Recheck every LLM token on this server.");
    // A token that isn't there is refused at the ask.
    assert!(ask_refused(&me, ACTION, json!({ "token": "acc_gone" })).contains("no such token"));
    let (s, _) = ask(&me, ACTION, json!({ "token": id, "workspace": me.handle }));
    assert!(s.starts_with("400"), "the workspace comes from the passport");
}
