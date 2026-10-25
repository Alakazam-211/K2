//! Agent App users and email invites (prd-agent-app-users-v1), headless:
//! the real dispatcher in process on a temp `$HOME`, over loopback.
//!
//! - S0: a platform pass (an agent can mint one for its own room) never
//!   gets a reset token from `/cli/skin/password/forgot`, and reaches only
//!   guests wholly inside its workspace; the owner still gets the token.
//! - S2: an agent adds App users of its own workspace's Apps only (one App
//!   grant, never the box login, stamped in the same transaction).
//! - S3: a guest an agent made (or whose password or email it set) is tied
//!   to its workspace: whatever the owner widens, sign-in, refresh and
//!   every live session stay at that workspace's access until the person's
//!   own reset; no chat pass; `GET /cli/skin/users` shows who waits.
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

// ── S3 ───────────────────────────────────────────────────────────────

async fn login(port: u16, body: Value) -> Resp {
    let b = body.to_string();
    tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(b))).await.unwrap()
}

/// The resolved policy of a live pass, as `room → caps` (in process: the
/// daemon and this test share skin.db).
fn policy_of(token: &str) -> Option<k2_core::skin::RoomPolicy> {
    k2_core::skin::resolve_skin_token(token).map(|p| p.room_policy)
}

fn tie_of(username: &str) -> Option<k2_core::skin::ties::Tie> {
    k2_core::skin::ties::get(&principal_id(username)).expect("tie lookup")
}

async fn user_row(port: u16, username: &str) -> Value {
    let r = get(port, "/cli/skin/users", OWNER, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()["users"].as_array().expect("users").iter().find(|u| u["username"] == username).cloned().unwrap_or_else(|| panic!("no {username}"))
}

/// POST with a Bearer credential and no Origin (a server-side caller).
async fn bearer_post(port: u16, path: &str, bearer: &str, body: Value) -> Resp {
    let (path, bearer, body) = (path.to_string(), bearer.to_string(), body.to_string());
    tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        s.set_read_timeout(Some(std::time::Duration::from_secs(30))).unwrap();
        let req = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nAuthorization: Bearer {bearer}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes()).unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        let text = String::from_utf8_lossy(&out).to_string();
        let status = text.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).expect("status");
        Resp { status, body: text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default() }
    })
    .await
    .unwrap()
}

fn rooms(p: &k2_core::skin::RoomPolicy) -> Vec<String> {
    p.keys().cloned().collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s3_a_tied_user_stays_at_the_agents_workspace_until_their_own_reset() {
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
    let pw = "s3cret-horse";

    // A role of the agent's workspace: Thread in sales.
    let role = format!("desk{tag}");
    owner_post(port, "/cli/skin/roles", json!({ "name": role, "roomAccess": [{ "handle": sales.handle, "caps": ["thread:read", "thread:post", "files:read"] }] })).await;

    // u1: made by the agent on its App, with a password it chose.
    let u1 = format!("u1{tag}");
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": u1, "app": shop, "role": role, "password": pw, "email": format!("{u1}@example.test") })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let t = tie_of(&u1).expect("tied");
    assert_eq!((t.workspace_id.as_str(), t.host_ok), (sales.id.as_str(), false));
    assert_eq!(t.reasons, vec!["created".to_string(), "password".to_string()]);
    assert!(t.set_by.starts_with("agent:"), "{t:?}");

    // u2: the owner's untied control with the same role on the same App.
    let u2 = format!("u2{tag}");
    owner_post(port, "/cli/skin/users", json!({ "username": u2, "app": shop, "role": role, "password": pw })).await;
    assert!(tie_of(&u2).is_none(), "the owner's add ties nobody");

    // T3.8 before any widening: tied, not pending.
    let row = user_row(port, &u1).await;
    assert_eq!((row["pendingReset"].clone(), row["tiedTo"].clone()), (json!(false), json!(sales.handle)), "{row}");
    let row2 = user_row(port, &u2).await;
    assert_eq!((row2["pendingReset"].clone(), row2["tiedTo"].clone()), (json!(false), Value::Null), "{row2}");

    // App A login: 200, sales only.
    let r = login(port, json!({ "username": u1, "password": pw, "app": shop })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let s1 = r.json()["token"].as_str().expect("token").to_string();
    assert_eq!(rooms(&policy_of(&s1).expect("live")), vec![sales.id.clone()]);
    let r = login(port, json!({ "username": u2, "password": pw, "app": shop })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let s2 = r.json()["token"].as_str().expect("token").to_string();

    // A device sign-in before the widening (T3.6).
    let r = login(port, json!({ "username": u1, "password": pw, "app": shop, "device": { "id": "phone-1", "name": "Phone" } })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let refresh = r.json()["refresh"].as_str().expect("device sign-in gives a refresh token").to_string();

    // T3.2: the owner adds the box login. Host login: the bad-password 401.
    owner_post(port, "/cli/skin/grants/host", json!({ "username": u1, "present": true })).await;
    let bad = login(port, json!({ "username": u1, "password": "wrong-password-1" })).await;
    let host = login(port, json!({ "username": u1, "password": pw })).await;
    assert_eq!((host.status, host.body.clone()), (bad.status, bad.body.clone()), "a tied host login looks exactly like a bad password");
    assert_eq!(host.status, 401);
    // T3.8: now pending.
    let row = user_row(port, &u1).await;
    assert_eq!(row["pendingReset"], json!(true), "{row}");

    // T3.3: the owner widens the role to ops (rewrites live sessions).
    owner_post(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ops.handle, "caps": ["thread:read"] })).await;
    assert_eq!(rooms(&policy_of(&s1).expect("still live")), vec![sales.id.clone()], "the existing tied session stays at sales");
    let mut both = vec![sales.id.clone(), ops.id.clone()];
    both.sort();
    assert_eq!(rooms(&policy_of(&s2).expect("control live")), both, "the untied control gets ops live");
    let r = login(port, json!({ "username": u1, "password": pw, "app": shop })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let s1b = r.json()["token"].as_str().expect("token").to_string();
    assert_eq!(rooms(&policy_of(&s1b).unwrap()), vec![sales.id.clone()], "a new login is clamped too");

    // T3.4: files:download on the role's sales line.
    owner_post(port, "/cli/skin/roles/room", json!({ "name": role, "handle": sales.handle, "caps": ["thread:read", "thread:post", "files:read", "files:download"] })).await;
    let tied = k2_core::skin::resolve_skin_token(&s1).expect("live");
    let control = k2_core::skin::resolve_skin_token(&s2).expect("live");
    assert!(!tied.has_cap_in_room(&sales.id, "files:download"), "person-only caps wait for the reset");
    assert!(tied.has_cap_in_room(&sales.id, "files:read"));
    assert!(control.has_cap_in_room(&sales.id, "files:download"), "the untied control has it");

    // T3.6: a refresh after the widening carries the clamped policy and is
    // never wider than the session it replaces.
    let r = tokio::task::spawn_blocking({
        let b = json!({ "refresh": refresh, "app": shop }).to_string();
        move || req_blocking(port, "POST", "/cli/skin/refresh", Some(b))
    })
    .await
    .unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    let fresh = r.json()["token"].as_str().expect("token").to_string();
    let fresh_pass = k2_core::skin::resolve_skin_token(&fresh).expect("refreshed pass");
    assert_eq!(rooms(&fresh_pass.room_policy), vec![sales.id.clone()]);
    assert!(!fresh_pass.has_cap_in_room(&sales.id, "files:download"));
    let stored: String = skin_db()
        .query_row("SELECT room_policy FROM tokens WHERE id = ?1", [fresh_pass.id.as_str()], |r| r.get(0))
        .unwrap();
    assert!(!stored.contains(&ops.id) && !stored.contains("files:download"), "the minted row itself is clamped: {stored}");

    // T3.5: a chat grant. The chat-only box login of a tied App-only user
    // is refused, and no chat pass from a tied App session.
    let u3 = format!("u3{tag}");
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": u3, "app": shop, "password": pw })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    k2_core::skin::insert_chat_grant_for_tests(&principal_id(&u3)).expect("chat grant");
    k2_core::skin::insert_chat_grant_for_tests(&principal_id(&u1)).expect("chat grant");
    let r = login(port, json!({ "username": u3, "password": pw })).await;
    assert_eq!(r.status, 401, "no chat-only box login while tied: {}", r.body);
    let r = bearer_post(port, "/cli/sew/pass", &s1b, json!({})).await;
    assert_eq!(r.status, 403, "{}", r.body);
    assert_eq!(r.json()["error"]["code"], "tied_pending_reset", "{}", r.body);

    // T3.7: the owner's box guest in sales; the agent sets its password
    // (tied, host_ok); the owner adds another workspace's App.
    let g1 = format!("g1{tag}");
    owner_post(port, "/cli/skin/users", json!({ "username": g1 })).await;
    owner_post(port, "/cli/skin/users/rooms", json!({ "username": g1, "rooms": [sales.handle] })).await;
    let r = post(port, "/cli/skin/users/password", &agent, json!({ "username": g1, "password": pw })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let t = tie_of(&g1).expect("tied by the password write");
    assert!(t.host_ok, "it held the box login then");
    assert_eq!(t.reasons, vec!["password".to_string()]);
    owner_post(port, "/cli/skin/users/rooms", json!({ "username": g1, "app": desk, "rooms": [ops.handle] })).await;
    let r = login(port, json!({ "username": g1, "password": pw })).await;
    assert_eq!(r.status, 200, "host login inside the ceiling: {}", r.body);
    let gtok = r.json()["token"].as_str().unwrap().to_string();
    assert_eq!(rooms(&policy_of(&gtok).unwrap()), vec![sales.id.clone()]);
    let r = login(port, json!({ "username": g1, "password": pw, "app": desk })).await;
    assert_eq!(r.status, 401, "another workspace's App waits for the reset: {}", r.body);
    assert_eq!(user_row(port, &g1).await["pendingReset"], json!(true));

    // An agent's email write ties too.
    let g2 = format!("g2{tag}");
    owner_post(port, "/cli/skin/users", json!({ "username": g2, "app": shop })).await;
    let r = post(port, "/cli/skin/users/email", &agent, json!({ "username": g2, "email": format!("{g2}@example.test") })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(tie_of(&g2).expect("tied by the email write").reasons, vec!["email".to_string()]);

    assert_eq!(k2_core::skin::ties::LOCK_ORDER_VIOLATIONS.load(std::sync::atomic::Ordering::SeqCst), 0);
}

/// T3.9: the App → workspace lookup never runs under the skin mutex, so a
/// thread holding the daemon DB mutex while it waits on the skin mutex
/// never deadlocks with a tied login.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s3_a_tied_login_never_waits_on_the_daemon_db_under_the_skin_lock() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let sales = seed_ws(&env, &format!("sales{tag}"));
    let shop = app_row(&sales, &format!("shop{tag}"));
    allow_manage(&sales, true);
    let agent = passport(&sales);
    let u = format!("lk{tag}");
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": u, "app": shop, "password": "s3cret-horse" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let holder = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                let db = k2_core::db::shared();
                let _conn = db.lock();
                // DB mutex held → wants the skin mutex.
                let _ = k2_core::skin::list_principals().expect("principals");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        })
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    for _ in 0..20 {
        let u = u.clone();
        let shop = shop.clone();
        let r = tokio::time::timeout(
            deadline.saturating_duration_since(std::time::Instant::now()),
            tokio::task::spawn_blocking(move || {
                let token = k2_core::skin::create_app_session_token(&u, &shop).expect("tied login");
                k2_core::skin::resolve_skin_token(&token.raw).expect("resolves")
            }),
        )
        .await;
        assert!(r.is_ok(), "a tied login deadlocked against a DB-then-skin holder");
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    holder.join().expect("holder");
    assert_eq!(k2_core::skin::ties::LOCK_ORDER_VIOLATIONS.load(std::sync::atomic::Ordering::SeqCst), 0);
}
