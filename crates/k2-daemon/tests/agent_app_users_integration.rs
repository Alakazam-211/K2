//! Agent App users and email invites (prd-agent-app-users-v1), headless:
//! the real dispatcher in process on a temp `$HOME`, over loopback.
//!
//! - S0: a platform pass (an agent can mint one for its own room) never
//!   gets a reset token from `/cli/skin/password/forgot`, and reaches only
//!   guests wholly inside its workspace; the owner still gets the token.
//! - S2: an agent adds App users of its own workspace's Apps only (one App
//!   grant, never the box login, stamped in the same transaction).
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

// ── S2 ───────────────────────────────────────────────────────────────

/// A published service row in `ws` (never started): an App.
fn app_row(ws: &Ws, name: &str) -> String {
    let dir = ws.path.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::published_services::insert(&conn, &ws.id, name, "node s.mjs", &dir.to_string_lossy(), 47_501, "local", "stopped", "cmd", "")
        .expect("insert service")
        .id
}

/// `(kind, target_id)` of every grant `username` holds.
fn grants_of(username: &str) -> Vec<(String, String)> {
    let db = skin_db();
    let mut stmt = db
        .prepare(
            "SELECT g.kind, g.target_id FROM grants g JOIN principals p ON p.id = g.subject_id
             WHERE g.subject_kind = 'principal' AND p.username = ?1 ORDER BY g.kind, g.target_id",
        )
        .unwrap();
    let rows = stmt.query_map([username], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
}

fn principal_exists(username: &str) -> bool {
    count("SELECT COUNT(*) FROM principals WHERE username = ?1", &[&username]) == 1
}

fn creator_of(username: &str) -> Option<String> {
    use rusqlite::OptionalExtension;
    skin_db()
        .query_row(
            "SELECT c.workspace_id FROM principal_agent_creators c JOIN principals p ON p.id = c.principal_id WHERE p.username = ?1",
            [username],
            |r| r.get(0),
        )
        .optional()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s2_an_agent_adds_app_users_of_its_own_apps_only() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let sales = seed_ws(&env, &format!("sales{tag}"));
    let ops = seed_ws(&env, &format!("ops{tag}"));
    let shop = app_row(&sales, &format!("shop{tag}"));
    let desk = app_row(&ops, &format!("desk{tag}"));
    allow_manage(&sales, true);
    let agent = passport(&sales);

    // T2.1: one App grant on its own App, no box login, stamped.
    let u1 = format!("u1{tag}");
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": u1, "app": shop, "email": format!("{u1}@example.test") })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.json()["actor"].as_str().expect("actor").starts_with("agent:"), "{}", r.body);
    assert_eq!(grants_of(&u1), vec![("app".to_string(), shop.clone())], "App only, never host");
    assert_eq!(creator_of(&u1).as_deref(), Some(sales.id.as_str()));

    // T2.2: no App → the teaching refusal, and no principal.
    let u2 = format!("u2{tag}");
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": u2 })).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);
    assert_eq!(
        r.json()["error"]["hint"],
        "Agents add App users only: name one of your workspace's Apps (k2 app user add <u> --app <app>). Box logins are the owner's (ask your human)."
    );
    assert!(!principal_exists(&u2));
    // `host` is the box login: the same refusal.
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": u2, "app": "host" })).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);
    assert!(!principal_exists(&u2));

    // T2.3: another workspace's App; a role reaching another room; a role
    // holding files:download. Nothing written in any case.
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": u2, "app": desk })).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);
    assert!(r.json()["error"]["hint"].as_str().unwrap().contains("isn't an App of your workspace"), "{}", r.body);
    let wide = format!("wide{tag}");
    owner_post(
        port,
        "/cli/skin/roles",
        json!({ "name": wide, "roomAccess": [{ "handle": sales.handle, "caps": ["thread:read"] }, { "handle": ops.handle, "caps": ["thread:read"] }] }),
    )
    .await;
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": u2, "app": shop, "role": wide })).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);
    let dl = format!("dl{tag}");
    owner_post(port, "/cli/skin/roles", json!({ "name": dl, "roomAccess": [{ "handle": sales.handle, "caps": ["thread:read", "files:read", "files:download"] }] })).await;
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": u2, "app": shop, "role": dl })).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);
    assert!(r.body.contains("files:download"), "{}", r.body);
    assert!(!principal_exists(&u2), "no row after any refusal");

    // A role of its own workspace is fine.
    let staff = format!("staff{tag}");
    owner_post(port, "/cli/skin/roles", json!({ "name": staff, "roomAccess": [{ "handle": sales.handle, "caps": ["thread:read", "thread:post"] }] })).await;
    let u3 = format!("u3{tag}");
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": u3, "app": shop, "role": staff })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(grants_of(&u3), vec![("app".to_string(), shop.clone())]);

    // T2.5: rooms on its own App user's App grant: 200; another
    // workspace's App: 403 and nothing written.
    let r = post(port, "/cli/skin/users/rooms", &agent, json!({ "username": u1, "app": shop, "rooms": [sales.handle] })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["rooms"], json!([sales.id]));
    let r = post(port, "/cli/skin/users/rooms", &agent, json!({ "username": u1, "app": desk, "rooms": [] })).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);
    assert_eq!(grants_of(&u1), vec![("app".to_string(), shop.clone())], "no grant on the other App");

    // T2.6: the owner's add without an App still writes the box login.
    let box_guest = format!("box{tag}");
    owner_post(port, "/cli/skin/users", json!({ "username": box_guest })).await;
    assert_eq!(grants_of(&box_guest), vec![("app".to_string(), "host".to_string())]);
    assert_eq!(creator_of(&box_guest), None, "the owner's guest carries no stamp");
    // And the owner may still name any App.
    let r = post(port, "/cli/skin/users", OWNER, json!({ "username": format!("o{tag}"), "app": desk })).await;
    assert_eq!(r.status, 200, "{}", r.body);
}
