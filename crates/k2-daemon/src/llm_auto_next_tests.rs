//! Auto-rotate end to end through the wallet: temp HOME, the process-wide
//! test DB (this test's Claude rows cleared before and after), fake Claude
//! credentials with a marker, a fake vendor, no network.

use std::collections::HashMap;

use chrono::Utc;
use k2_core::llm_accounts::identity::AccountIdentity;
use k2_core::llm_accounts::store;
use rusqlite::params;
use serde_json::{json, Value};

use super::*;
use crate::llm_accounts_routes::{self as r, RUN_SWITCH_CALLS};
use crate::sidecar_routes::Caller;

const MARKER: &str = "K2TEST_SECRET_MARKER";

/// Names the account from the access token's tag (`…-at-<tag>`).
struct FakeVendor;

impl wallet::Refresher for FakeVendor {
    fn refresh(&self, _tool: Tool, _id: &str) -> Result<(), String> {
        panic!("no refresh expected: every fake token is far from expiry");
    }
    fn identify(&self, _tool: Tool, b: &[u8]) -> Option<AccountIdentity> {
        let v: Value = serde_json::from_slice(b).ok()?;
        let at = v["claudeAiOauth"]["accessToken"].as_str()?;
        let who = at.rsplit("-at-").next()?.to_string();
        Some(AccountIdentity { account_uuid: Some(format!("uuid-{who}")), email: Some(format!("{who}@example.test")), org_uuid: None })
    }
}

fn cred(tag: &str) -> String {
    format!(r#"{{"claudeAiOauth":{{"accessToken":"{MARKER}-at-{tag}","refreshToken":"{MARKER}-rt-{tag}","expiresAt":4102444800000}}}}"#)
}

fn with_conn<T>(f: impl FnOnce(&rusqlite::Connection) -> T) -> T {
    let db = k2_core::db::shared();
    let conn = db.lock();
    f(&conn)
}

fn clear_rows() {
    with_conn(|conn| {
        conn.execute("DELETE FROM llm_active WHERE tool = 'claude'", []).unwrap();
        conn.execute("DELETE FROM llm_accounts WHERE tool = 'claude'", []).unwrap();
        conn.execute("DELETE FROM llm_auto_next", []).unwrap();
    });
    NOTICES.lock().clear();
    RUN_SWITCH_CALLS.with(|c| c.set(0));
}

/// Clears this test's rows when it ends, even on a failed assert.
struct Rows;
impl Drop for Rows {
    fn drop(&mut self) {
        clear_rows();
    }
}

/// Live = `a` (imported, the server default); each of `others` signed in
/// to its own slot. Returns the ids in that order.
fn setup(home: &std::path::Path, others: &[&str]) -> Vec<String> {
    clear_rows();
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::write(home.join(".claude/.credentials.json"), cred("a")).unwrap();
    std::fs::write(home.join(".claude.json"), r#"{"oauthAccount":{"accountUuid":"uuid-a","emailAddress":"a@example.test"}}"#).unwrap();
    let a = with_conn(|conn| wallet::import_live(conn, Tool::Claude, None)).unwrap().expect("imported");
    let mut ids = vec![a.id];
    for tag in others {
        let e = with_conn(|conn| wallet_core::create(conn, Tool::Claude, tag, None)).unwrap();
        let dir = store::slot_dir(Tool::Claude, &e.id);
        store::ensure_private_dir(&dir).unwrap();
        std::fs::write(dir.join(".credentials.json"), cred(tag)).unwrap();
        std::fs::write(
            dir.join(".claude.json"),
            format!(r#"{{"oauthAccount":{{"accountUuid":"uuid-{tag}","emailAddress":"{tag}@example.test"}}}}"#),
        )
        .unwrap();
        with_conn(|conn| {
            wallet_core::update_meta(conn, &e.id, &wallet_core::MetaUpdate { state: Some(state::SIGNED_IN.into()), ..Default::default() })
        })
        .unwrap();
        ids.push(e.id);
    }
    ids
}

/// A token's last reading: its Session window at `percent`.
fn set_usage(id: &str, percent: f64) {
    let resets = (Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
    let row = su::HarnessUsage {
        harness: "claude".into(),
        plan: "Max".into(),
        windows: vec![su::window("five_hour", "Session", "session", percent / 100.0, resets, Some(5 * 3600))],
        checked_at: Utc::now().to_rfc3339(),
        status: "ok".into(),
        ..Default::default()
    };
    let json = serde_json::to_string(&row).unwrap();
    with_conn(|conn| conn.execute("UPDATE llm_accounts SET usage_json = ?2 WHERE id = ?1", params![id, json])).unwrap();
}

fn active() -> Option<String> {
    with_conn(|conn| wallet_core::active_id(conn, Tool::Claude)).unwrap()
}

fn on(person: &str) {
    with_conn(|conn| auto_next::turn_on(conn, Tool::Claude, person, 1)).unwrap();
}

fn tick_at(now: chrono::DateTime<Utc>) -> Vec<(String, Outcome)> {
    tick_with(&FakeVendor, now).expect("no other tick running")
}

fn claude(out: &[(String, Outcome)]) -> Outcome {
    out.iter().find(|(t, _)| t == "claude").map(|(_, o)| o.clone()).expect("a claude outcome")
}

fn switch_calls() -> usize {
    RUN_SWITCH_CALLS.with(|c| c.get())
}

fn call(path: &str, caller: Caller, body: &str) -> (String, Value) {
    let resp = r::handle(path, caller, body.as_bytes(), &HashMap::new(), "test");
    (resp.status.to_string(), serde_json::from_str(&resp.body).expect("json body"))
}

fn login(name: &str, role: k2_core::connect_users::Role) -> Caller {
    k2_core::connect_users::add_user(name, "password123").expect("add_user");
    if role != k2_core::connect_users::Role::Member {
        k2_core::connect_users::set_role(name, role).expect("set_role");
    }
    Caller::Login { username: name.into() }
}

fn agent() -> Caller {
    Caller::Agent(crate::sidecar_routes::AgentCaller { project_id: "ws-test".into(), map_key: None, args: vec![], sandboxed: false })
}

#[test]
fn auto_next_is_off_by_default_and_nothing_switches() {
    let home = crate::test_support::TempHome::new();
    let _rows = Rows;
    let ids = setup(home.path(), &["b"]);
    set_usage(&ids[0], 100.0);
    assert!(tick_at(Utc::now()).is_empty(), "off by default: no tool to look at");
    assert_eq!(active().as_deref(), Some(ids[0].as_str()));
    assert_eq!(switch_calls(), 0);
    let (st, v) = call(r::AUTO_NEXT, Caller::Owner, "");
    assert_eq!(st, "200 OK", "{v}");
    for t in v["tools"].as_array().expect("tools") {
        assert_eq!(t["on"], false, "{t}");
    }
    assert!(v["note"].as_str().unwrap().contains("provider's terms"), "{v}");
    let (_, l) = call(r::LIST, Caller::Owner, "");
    let c = l["tools"].as_array().unwrap().iter().find(|t| t["tool"] == "claude").unwrap().clone();
    assert_eq!(c["autoNext"]["on"], false, "{c}");
}

#[test]
fn auto_next_an_agent_cannot_turn_it_on_and_a_person_needs_a_confirm() {
    use k2_core::connect_users::Role;
    let _home = crate::test_support::TempHome::new();
    let _rows = Rows;
    clear_rows();
    let vault = crate::vault::ensure_test_vault();
    let on_body = |tok: Option<&str>| match tok {
        Some(t) => json!({"tool": "claude", "on": true, "confirmToken": t}).to_string(),
        None => json!({"tool": "claude", "on": true}).to_string(),
    };
    // An agent passport (and a K2 terminal): refused, even with a confirm.
    let tok = vault.confirm.mint(auto_next::CONFIRM_ACTION, Some("claude"), "owner", "local-auth");
    let (st, v) = call(r::AUTO_NEXT_SET, agent(), &on_body(Some(&tok)));
    assert_eq!((st.as_str(), &v["error"]["code"]), ("403 Forbidden", &json!("owner_only")), "{v}");
    let (st, v) = call(r::AUTO_NEXT, agent(), "");
    assert_eq!((st.as_str(), &v["error"]["code"]), ("403 Forbidden", &json!("owner_only")), "{v}");
    // A Member can't (it presses Next, which is Admin and above).
    let member = login("autonext_member", Role::Member);
    let (st, v) = call(r::AUTO_NEXT_SET, member, &on_body(None));
    assert_eq!((st.as_str(), &v["error"]["code"]), ("403 Forbidden", &json!("admin_required")), "{v}");
    // The owner token alone isn't enough: it needs the person's confirm,
    // for this action and this tool, minted for this person.
    let (st, v) = call(r::AUTO_NEXT_SET, Caller::Owner, &on_body(None));
    assert_eq!(st, "403 Forbidden", "{v}");
    assert_eq!(v["error"]["code"], "vault_confirm_required");
    assert_eq!((v["error"]["action"].clone(), v["error"]["secretId"].clone()), (json!(auto_next::CONFIRM_ACTION), json!("claude")));
    let wrong_tool = vault.confirm.mint(auto_next::CONFIRM_ACTION, Some("codex"), "owner", "local-auth");
    let wrong_action = vault.confirm.mint("reveal", Some("claude"), "owner", "local-auth");
    let someone_else = vault.confirm.mint(auto_next::CONFIRM_ACTION, Some("claude"), "connect:pat", "connect-password");
    for t in [wrong_tool.as_str(), wrong_action.as_str(), someone_else.as_str(), "k2vc_madeup"] {
        let (st, v) = call(r::AUTO_NEXT_SET, Caller::Owner, &on_body(Some(t)));
        assert_eq!((st.as_str(), &v["error"]["code"]), ("403 Forbidden", &json!("vault_confirm_required")), "{t}: {v}");
    }
    assert_eq!(with_conn(|c| auto_next::get(c, Tool::Claude)).unwrap(), None, "still off");
    // The person's own confirm turns it on, once.
    let mine = vault.confirm.mint(auto_next::CONFIRM_ACTION, Some("claude"), "owner", "local-auth");
    let (st, v) = call(r::AUTO_NEXT_SET, Caller::Owner, &on_body(Some(&mine)));
    assert_eq!(st, "200 OK", "{v}");
    let c = v["tools"].as_array().unwrap().iter().find(|t| t["tool"] == "claude").unwrap().clone();
    assert_eq!((c["on"].clone(), c["by"].clone(), c["byYou"].clone()), (json!(true), json!("owner-token"), json!(true)), "{c}");
    assert_eq!(v["tools"].as_array().unwrap().iter().filter(|t| t["on"] == true).count(), 1, "per tool: {v}");
    // An agent reads that it's on from the list, not who turned it on.
    let (_, l) = call(r::LIST, agent(), "");
    let lc = l["tools"].as_array().unwrap().iter().find(|t| t["tool"] == "claude").unwrap().clone();
    assert_eq!((lc["autoNext"]["on"].clone(), lc["autoNext"]["by"].clone()), (json!(true), Value::Null), "{lc}");
    // An agent can't turn it off either; a person can, with no confirm.
    let off = json!({"tool": "claude", "on": false}).to_string();
    let (st, _) = call(r::AUTO_NEXT_SET, agent(), &off);
    assert_eq!(st, "403 Forbidden");
    let admin = login("autonext_admin", Role::Admin);
    let (st, v) = call(r::AUTO_NEXT_SET, admin, &off);
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(with_conn(|c| auto_next::get(c, Tool::Claude)).unwrap(), None);
    // GET on the mutating route is refused by the dispatcher (POST-only row).
    assert!(r::is_post_route(r::AUTO_NEXT_SET) && !r::is_post_route(r::AUTO_NEXT));
}

#[test]
fn auto_next_at_98_percent_does_nothing() {
    let home = crate::test_support::TempHome::new();
    let _rows = Rows;
    let ids = setup(home.path(), &["b"]);
    on("owner-token");
    set_usage(&ids[0], 98.0);
    set_usage(&ids[1], 5.0);
    assert_eq!(claude(&tick_at(Utc::now())), Outcome::Nothing);
    assert_eq!(active().as_deref(), Some(ids[0].as_str()));
    assert_eq!(switch_calls(), 0, "Next was never pressed");
}

#[test]
fn auto_next_at_99_percent_switches_once_through_next_and_does_not_flap() {
    let home = crate::test_support::TempHome::new();
    let _rows = Rows;
    let ids = setup(home.path(), &["b"]);
    on("owner-token");
    set_usage(&ids[0], 99.0);
    set_usage(&ids[1], 10.0);
    let t0 = Utc::now();
    let o = claude(&tick_at(t0));
    let Outcome::Switched { to, line } = o else { panic!("expected a switch, got {o:?}") };
    assert_eq!(to, ids[1]);
    assert!(line.starts_with("Switched to b automatically: ") && line.ends_with(" reached 99%"), "{line}");
    assert_eq!(active().as_deref(), Some(ids[1].as_str()));
    assert_eq!(switch_calls(), 1, "one press of the Next path");
    // The same switch as Next: B is live in the tool's own store, and the
    // default records who made it.
    assert_eq!(std::fs::read_to_string(home.path().join(".claude/.credentials.json")).unwrap(), cred("b"));
    let by: String =
        with_conn(|c| c.query_row("SELECT switched_by FROM llm_active WHERE tool = 'claude'", [], |r| r.get(0))).unwrap();
    assert_eq!(by, "auto-next:owner-token");
    let s = with_conn(|c| auto_next::get(c, Tool::Claude)).unwrap().expect("on");
    assert_eq!((s.last_line.as_deref(), s.last_switch_at), (Some(line.as_str()), Some(t0.timestamp())));
    // B reaches its limit too, A has room again: inside the cooldown
    // nothing moves.
    set_usage(&ids[1], 100.0);
    set_usage(&ids[0], 0.0);
    for secs in [1, 60, auto_next::COOLDOWN_SECS - 1] {
        assert_eq!(claude(&tick_at(t0 + chrono::Duration::seconds(secs))), Outcome::Cooldown, "+{secs}s");
    }
    assert_eq!(active().as_deref(), Some(ids[1].as_str()));
    assert_eq!(switch_calls(), 1, "no flap");
    // After the cooldown it may move again.
    let o = claude(&tick_at(t0 + chrono::Duration::seconds(auto_next::COOLDOWN_SECS)));
    assert!(matches!(&o, Outcome::Switched { to, .. } if to == &ids[0]), "{o:?}");
    assert_eq!(switch_calls(), 2);
}

#[test]
fn auto_next_skips_tokens_at_their_limit_and_backing_off() {
    let home = crate::test_support::TempHome::new();
    let _rows = Rows;
    let ids = setup(home.path(), &["b", "c", "d"]);
    on("owner-token");
    set_usage(&ids[0], 99.5);
    // Next's order after `a` is the label order; make every other one but
    // one unusable: one at its limit, one whose usage check backs off.
    set_usage(&ids[1], 99.0);
    set_usage(&ids[2], 3.0);
    set_usage(&ids[3], 3.0);
    let failed = su::HarnessUsage { harness: "claude".into(), error: Some("HTTP 500".into()), ..Default::default() };
    su::note_outcome(&format!("acct:{}", ids[2]), &failed, Utc::now());
    let o = claude(&tick_at(Utc::now()));
    assert!(matches!(&o, Outcome::Switched { to, .. } if to == &ids[3]), "b is at its limit, c backs off: {o:?}");
    assert_eq!(active().as_deref(), Some(ids[3].as_str()));
    su::tests_clear_backoffs();
}

#[test]
fn auto_next_with_every_subscription_at_its_limit_switches_nowhere_and_says_so_once() {
    let home = crate::test_support::TempHome::new();
    let _rows = Rows;
    let ids = setup(home.path(), &["b"]);
    on("owner-token");
    set_usage(&ids[0], 99.0);
    set_usage(&ids[1], 100.0);
    assert_eq!(claude(&tick_at(Utc::now())), Outcome::AllAtLimit { told: true });
    assert_eq!(claude(&tick_at(Utc::now())), Outcome::AllAtLimit { told: false }, "one line per episode");
    assert_eq!(active().as_deref(), Some(ids[0].as_str()), "no switch");
    assert_eq!(switch_calls(), 0);
    let notices = NOTICES.lock().clone();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].starts_with("All your Claude subscriptions are at their limit"), "{notices:?}");
    let s = with_conn(|c| auto_next::get(c, Tool::Claude)).unwrap().expect("on");
    assert_eq!(s.last_line.as_deref(), Some("All your Claude subscriptions are at their limit"));
    // Below the limit again: a new episode tells again.
    set_usage(&ids[0], 20.0);
    assert_eq!(claude(&tick_at(Utc::now())), Outcome::Nothing);
    set_usage(&ids[0], 99.0);
    assert_eq!(claude(&tick_at(Utc::now())), Outcome::AllAtLimit { told: true });
    assert_eq!(NOTICES.lock().len(), 2);
}

#[test]
fn auto_next_never_moves_a_pinned_subscription_into_the_default() {
    let home = crate::test_support::TempHome::new();
    let _rows = Rows;
    let ids = setup(home.path(), &["b"]);
    on("owner-token");
    set_usage(&ids[0], 99.0);
    set_usage(&ids[1], 1.0);
    // B is pinned to a workspace: it is live there, so never also the default.
    with_conn(|conn| {
        conn.execute("INSERT OR IGNORE INTO projects (id, name, path) VALUES ('ws-autonext', 'autonext', '/tmp/ws-autonext')", []).unwrap();
        k2_core::llm_accounts::pins::pin(conn, k2_core::llm_accounts::pins::ScopeKind::Workspace, "ws-autonext", Some(Tool::Claude), &ids[1], None)
    })
    .unwrap();
    assert_eq!(claude(&tick_at(Utc::now())), Outcome::AllAtLimit { told: true });
    assert_eq!(active().as_deref(), Some(ids[0].as_str()));
    with_conn(|conn| conn.execute("DELETE FROM llm_account_pins WHERE account_id = ?1", params![ids[1]])).unwrap();
}

#[test]
fn auto_next_stops_when_its_person_can_no_longer_change_the_default() {
    use k2_core::connect_users::Role;
    let home = crate::test_support::TempHome::new();
    let _rows = Rows;
    let ids = setup(home.path(), &["b"]);
    let _ = login("autonext_was_admin", Role::Member);
    on("login:autonext_was_admin");
    set_usage(&ids[0], 100.0);
    set_usage(&ids[1], 1.0);
    assert_eq!(claude(&tick_at(Utc::now())), Outcome::PersonCannotAct);
    assert_eq!(active().as_deref(), Some(ids[0].as_str()));
    k2_core::connect_users::set_role("autonext_was_admin", Role::Admin).unwrap();
    assert!(matches!(claude(&tick_at(Utc::now())), Outcome::Switched { .. }));
}

#[test]
fn auto_next_and_the_next_button_take_the_same_path() {
    let _home = crate::test_support::TempHome::new();
    let _rows = Rows;
    clear_rows();
    // The route's Next arm goes through run_switch (here it answers
    // no_next: no subscription at all), as the loop does.
    let (st, v) = call(r::NEXT, Caller::Owner, r#"{"tool":"claude"}"#);
    assert_eq!((st.as_str(), &v["error"]["code"]), ("404 Not Found", &json!("no_next")), "{v}");
    assert_eq!(switch_calls(), 1);
    let err = r::run_next(Tool::Claude, "auto-next:owner-token", "test", &FakeVendor, &[]).expect_err("nothing to move to");
    assert_eq!(err.code(), "not_found");
    assert_eq!(switch_calls(), 2);
}
