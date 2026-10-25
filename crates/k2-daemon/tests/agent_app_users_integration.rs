//! Agent App users and email invites (prd-agent-app-users-v1), headless:
//! the real dispatcher in process on a temp `$HOME`, over loopback.
//!
//! - S0: a platform pass (an agent can mint one for its own room) never
//!   gets a reset token from `/cli/skin/password/forgot`, and reaches only
//!   guests wholly inside its workspace; the owner still gets the token.
//!
//! Never the real `~/.k2`, never the keychain, never a real mail send.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;
use serde_json::{json, Value};

async fn owner_post(port: u16, path: &str, body: Value) -> Value {
    let r = post(port, path, OWNER, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

/// A read-only look at skin.db (the test's temp `$HOME`).
fn skin_db() -> rusqlite::Connection {
    rusqlite::Connection::open(k2_core::paths::k2_home().join("skin.db")).expect("open skin.db")
}

fn count(sql: &str, args: &[&dyn rusqlite::ToSql]) -> i64 {
    skin_db().query_row(sql, args, |r| r.get(0)).unwrap_or_else(|e| panic!("{sql}: {e}"))
}

fn principal_id(username: &str) -> String {
    skin_db()
        .query_row("SELECT id FROM principals WHERE username = ?1", [username], |r| r.get(0))
        .unwrap_or_else(|e| panic!("principal {username}: {e}"))
}

fn reset_rows(username: &str) -> i64 {
    count(
        "SELECT COUNT(*) FROM password_reset_tokens t JOIN principals p ON p.id = t.principal_id WHERE p.username = ?1",
        &[&username],
    )
}

/// An agent of `ws` mints a platform pass for its own room.
async fn agent_pass(port: u16, ws: &Ws, agent: &str, name: &str) -> String {
    let r = post(port, "/cli/skin-tokens", agent, json!({ "name": name, "caps": ["thread:read"], "rooms": [ws.handle] })).await;
    assert_eq!(r.status, 200, "agent mints a pass for its own room: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

async fn forgot(port: u16, token: &str, username: &str) -> Resp {
    post(port, "/cli/skin/password/forgot", token, json!({ "username": username })).await
}

fn assert_bare_ok(r: &Resp, what: &str) {
    assert_eq!(r.status, 200, "{what}: {}", r.body);
    let v = r.json();
    assert_eq!(v, json!({ "ok": true }), "{what}: a pass gets only {{ok:true}}, never a token: {}", r.body);
    assert!(!r.body.contains("token"), "{what}: {}", r.body);
}

// ── S0 ───────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s0_a_pass_never_gets_a_reset_token_and_reaches_only_its_own_guests() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let sales = seed_ws(&env, &format!("sales{tag}"));
    let ops = seed_ws(&env, &format!("ops{tag}"));
    allow_manage(&sales, true);
    let agent = passport(&sales);

    // The owner's guest in another workspace, and the owner's own empty
    // box guest: both have a password and an email.
    let victim = format!("victim{tag}");
    owner_post(port, "/cli/skin/users", json!({ "username": victim, "email": format!("{victim}@example.test") })).await;
    owner_post(port, "/cli/skin/users/password", json!({ "username": victim, "password": "s3cret-horse" })).await;
    owner_post(port, "/cli/skin/users/rooms", json!({ "username": victim, "rooms": [ops.handle] })).await;
    let bare = format!("bare{tag}");
    owner_post(port, "/cli/skin/users", json!({ "username": bare, "email": format!("{bare}@example.test") })).await;
    owner_post(port, "/cli/skin/users/password", json!({ "username": bare, "password": "s3cret-horse" })).await;

    // A guest wholly inside sales (its rooms), password and email set.
    let own = format!("own{tag}");
    owner_post(port, "/cli/skin/users", json!({ "username": own, "email": format!("{own}@example.test") })).await;
    owner_post(port, "/cli/skin/users/password", json!({ "username": own, "password": "s3cret-horse" })).await;
    owner_post(port, "/cli/skin/users/rooms", json!({ "username": own, "rooms": [sales.handle] })).await;

    let pass = agent_pass(port, &sales, &agent, &format!("p{tag}")).await;

    // Another workspace's guest, the owner's empty guest, by name or by
    // email: the bare {ok:true}, and nothing minted.
    for (who, presented) in [
        (&victim, victim.clone()),
        (&victim, format!("{victim}@example.test")),
        (&bare, bare.clone()),
    ] {
        let r = forgot(port, &pass, &presented).await;
        assert_bare_ok(&r, &format!("pass forgot {presented}"));
        assert_eq!(reset_rows(who), 0, "no reset token minted for {who}");
    }

    // Its own guest: still never a token in the answer.
    let r = forgot(port, &pass, &own).await;
    assert_bare_ok(&r, "pass forgot own guest");
    assert_eq!(reset_rows(&own), 0, "nothing usable minted for the pass");

    // An out-of-reach pass call never spent the guest's 60 s throttle:
    // the owner's forgot for the victim still mints, and gets the token.
    let r = forgot(port, OWNER, &victim).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    let token = v["token"].as_str().expect("the owner gets the token");
    assert!(!token.starts_with("k2skn_"));
    assert_eq!(v["email"], format!("{victim}@example.test"));
    assert_eq!(reset_rows(&victim), 1);

    // The agent's passport itself is still refused outright.
    let r = forgot(port, &agent, &own).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.code(), "owner_only");

    // A guest's own session is still 403.
    let login = tokio::task::spawn_blocking({
        let b = json!({ "username": own, "password": "s3cret-horse" }).to_string();
        move || req_blocking(port, "POST", "/cli/skin/login", Some(b))
    })
    .await
    .unwrap();
    assert_eq!(login.status, 200, "{}", login.body);
    let sess = login.json()["token"].as_str().expect("session").to_string();
    let r = forgot(port, &sess, &own).await;
    assert_eq!(r.status, 403, "{}", r.body);
    let _ = principal_id(&own);
}
