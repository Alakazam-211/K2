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
    // 0.46.4 review: an agent never manages a box login, its own room or
    // not: refused before any write or tie.
    let r = post(port, "/cli/skin/users/password", &agent, json!({ "username": g1, "password": pw })).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);
    assert!(r.body.contains("box login"), "{}", r.body);
    assert!(tie_of(&g1).is_none(), "refused before any tie");
    // A tie a password write made before 0.46.4 still holds the login to
    // the ceiling below.
    k2_core::skin::set_principal_password(&g1, Some(pw)).expect("owner sets it");
    k2_core::skin::creators::tie(&principal_id(&g1), &sales.id, "agent:legacy", k2_core::skin::ties::REASON_PASSWORD).expect("tie");
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

    // An agent sets the email only of a person its agent added (review
    // item 10); that write ties too. The owner's person: refused, untied.
    let g2 = format!("g2{tag}");
    owner_post(port, "/cli/skin/users", json!({ "username": g2, "app": shop })).await;
    let r = post(port, "/cli/skin/users/email", &agent, json!({ "username": g2, "email": format!("{g2}@example.test") })).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);
    assert!(tie_of(&g2).is_none(), "the owner's person stays untied");
    let a2 = format!("a2{tag}");
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": a2, "app": shop })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/skin/users/email", &agent, json!({ "username": a2, "email": format!("{a2}@example.test") })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(tie_of(&a2).expect("tied").reasons, vec!["created".to_string(), "email".to_string()]);

    assert_eq!(k2_core::skin::ties::LOCK_ORDER_VIOLATIONS.load(std::sync::atomic::Ordering::SeqCst), 0);
}

/// T3.9: the App → workspace lookup never runs under the skin mutex. While
/// one thread holds the daemon DB mutex, a tied login waits for it with
/// the skin mutex free (another thread reads skin.db meanwhile), and
/// finishes once the DB is released; the probe counts no violation.
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
    for round in 0..3 {
        let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let db = k2_core::db::shared();
            let _conn = db.lock();
            held_tx.send(()).expect("held");
            release_rx.recv_timeout(std::time::Duration::from_secs(10)).expect("release");
        });
        held_rx.recv_timeout(std::time::Duration::from_secs(5)).expect("the holder took the DB mutex");
        let login = {
            let (u, shop) = (u.clone(), shop.clone());
            tokio::task::spawn_blocking(move || {
                let token = k2_core::skin::create_app_session_token(&u, &shop).expect("tied login");
                k2_core::skin::resolve_skin_token(&token.raw).expect("resolves")
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        // The login waits on the DB, never holding the skin mutex: skin.db
        // stays readable from another thread.
        let read = tokio::time::timeout(std::time::Duration::from_secs(3), tokio::task::spawn_blocking(|| k2_core::skin::ties::list().expect("ties")))
            .await;
        assert!(read.is_ok(), "round {round}: the skin mutex was held while the login waited on the DB");
        release_tx.send(()).expect("release");
        holder.join().expect("holder");
        let pass = tokio::time::timeout(std::time::Duration::from_secs(5), login)
            .await
            .unwrap_or_else(|_| panic!("round {round}: the tied login never finished"))
            .expect("login task");
        assert_eq!(pass.rooms, Vec::<String>::new(), "a role-less App grant: Thread-dark, inside the ceiling");
    }
    assert_eq!(k2_core::skin::ties::LOCK_ORDER_VIOLATIONS.load(std::sync::atomic::Ordering::SeqCst), 0);
}

// ── S4 / S5: links and mail ──────────────────────────────────────────

/// Every hosted submit, in place of the mail server (no real sends).
#[derive(Default)]
struct Recorder(std::sync::Mutex<Vec<k2_daemon::mail::send::OutboundMessage>>);

impl k2_daemon::mail::send::SubmitBackend for Recorder {
    fn submit(&self, _account_id: &str, msg: &k2_daemon::mail::send::OutboundMessage) -> Result<(), String> {
        self.0.lock().unwrap().push(msg.clone());
        Ok(())
    }
}

fn recorder() -> std::sync::Arc<Recorder> {
    static R: std::sync::OnceLock<std::sync::Arc<Recorder>> = std::sync::OnceLock::new();
    let r = R.get_or_init(|| std::sync::Arc::new(Recorder::default())).clone();
    let b: std::sync::Arc<dyn k2_daemon::mail::send::SubmitBackend + Send + Sync> = r.clone();
    k2_daemon::app_user_invites::set_backend_override(Some(b));
    r
}

/// The mails sent to `to` so far.
fn mails_to(r: &Recorder, to: &str) -> Vec<k2_daemon::mail::send::OutboundMessage> {
    r.0.lock().unwrap().iter().filter(|m| m.to.iter().any(|t| t == to)).cloned().collect()
}

/// The `k2inv_…` token in a mail body.
fn token_in(body: &str) -> String {
    let i = body.find("#k2inv_").unwrap_or_else(|| panic!("no #k2inv_ link in {body}"));
    body[i + 1..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect()
}

/// A hosted mailbox `bot@<domain>` for `ws` (direct send), and its mode.
fn seed_mail(ws: &Ws, domain: &str, mode: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let addr = format!("bot@{domain}");
    conn.execute(
        "INSERT INTO mail_domains (id, domain, send_mode, status, created_at) VALUES (?1, ?2, 'direct', 'verified', 100)",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), domain],
    )
    .expect("domain");
    conn.execute(
        "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, owner_project_id, status, created_at)
         VALUES (?1, ?2, 'dom-x', ?3, ?4, 'active', 100)",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), addr, format!("acct-{domain}"), ws.id],
    )
    .expect("address");
    conn.execute("UPDATE projects SET mail_agent_send = ?2 WHERE id = ?1", rusqlite::params![ws.id, mode]).expect("mode");
    addr
}

fn set_mode(ws: &Ws, mode: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("UPDATE projects SET mail_agent_send = ?2 WHERE id = ?1", rusqlite::params![ws.id, mode]).expect("mode");
}

fn outbound_rows(from: &str) -> Vec<(String, String, String)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut st = conn.prepare("SELECT id, status, body_ref FROM mail_outbound WHERE from_address = ?1 ORDER BY created_at, id").unwrap();
    let rows = st.query_map([from], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
}

fn invite_rows(username: &str) -> Vec<k2_core::skin::invites::InviteRow> {
    k2_core::skin::invites::rows_for(&principal_id(username)).expect("invite rows")
}

async fn reset_with(port: u16, token: &str, password: &str) -> Resp {
    let b = json!({ "token": token, "password": password }).to_string();
    tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/password/reset", Some(b))).await.unwrap()
}

const RESET_FAILED: &str = r#"{"error":"invalid or expired reset token"}"#;

/// The box's front door URL (direct mode, not applied: no Caddy).
async fn set_front_door(port: u16, url: &str) {
    owner_post(port, "/cli/skin/front-door", json!({ "mode": "direct", "url": url, "apply": false })).await;
}

/// A hosted mailbox no workspace's agent can read (a box/system sender).
fn seed_system_sender(domain: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let addr = format!("noreply@{domain}");
    conn.execute(
        "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, owner_project_id, status, created_at)
         VALUES (?1, ?2, 'dom-x', ?3, 'box-system', 'active', 100)",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), addr, format!("acct-sys-{domain}")],
    )
    .expect("system sender");
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s5_an_agent_invites_through_its_own_mail_under_its_governance() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let rec = recorder();
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let sales = seed_ws(&env, &format!("sales{tag}"));
    let ops = seed_ws(&env, &format!("ops{tag}"));
    let shop = app_row(&sales, &format!("shop{tag}"));
    let desk = app_row(&ops, &format!("desk{tag}"));
    allow_manage(&sales, true);
    let agent = passport(&sales);
    let domain = format!("inv{tag}.example.test");
    let from = seed_mail(&sales, &domain, "on");

    // POST only, all three new routes; K2's own page is a GET.
    for path in ["/cli/skin/users/invite", "/cli/skin/users/reset-link", "/cli/skin/access/invites"] {
        let r = get(port, path, OWNER, "").await;
        assert_eq!(r.status, 405, "GET {path}: {}", r.body);
    }
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "GET", "/cli/skin/password/set", None)).await.unwrap();
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("Set your password") && r.body.contains("/cli/skin/password/reset"), "{}", r.body);

    // T5.7: no set-password page (no App page, no front door) → 409.
    let u3 = format!("u3{tag}");
    let e3 = format!("{u3}@example.test");
    let r = post(port, "/cli/skin/users/invite", &agent, json!({ "username": u3, "app": shop, "email": e3 })).await;
    assert_eq!((r.status, r.code()), (409, "no_set_password_page".to_string()), "{}", r.body);
    assert!(!principal_exists(&u3), "nothing written");

    // The App's own page: the owner sets it (an agent without switch 2
    // can't); https on the App's own domain or K2's, or exact localhost.
    let door = format!("https://door{tag}.example.test");
    set_front_door(port, &door).await;
    let r = post(port, "/cli/skin/access/invites", &agent, json!({ "app": shop, "setPasswordUrl": format!("{door}/x") })).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);
    let page = format!("{door}/apps/shop/set-password");
    for bad in [
        json!({ "app": shop, "setPasswordUrl": "https://evil.example.test/set" }),
        json!({ "app": shop, "setPasswordUrl": "http://localhost.attacker.tld/set" }),
        json!({ "app": shop, "setPasswordUrl": format!("{door}/set#a") }),
        json!({ "app": shop, "setPasswordUrl": format!("http://door{tag}.example.test/set") }),
        json!({ "app": shop, "inviteTtl": "5m" }),
        json!({ "app": shop, "inviteTtl": "31d" }),
        json!({ "app": shop, "inviteBody": "click {link}" }),
        json!({ "app": shop, "inviteBody": "see https://evil.example.test" }),
        json!({ "app": shop, "inviteBody": "see www.evil.example" }),
        json!({ "app": shop, "inviteSubject": "Hi from HTTP://evil" }),
    ] {
        let r = post(port, "/cli/skin/access/invites", OWNER, bad.clone()).await;
        assert_eq!(r.status, 400, "{bad}: {}", r.body);
    }
    let r = post(port, "/cli/skin/access/invites", OWNER, json!({ "app": shop, "setPasswordUrl": "http://localhost:3000/set" })).await;
    assert_eq!(r.status, 200, "a local App's page: {}", r.body);
    let r = post(port, "/cli/skin/access/invites", OWNER, json!({ "app": shop, "setPasswordUrl": page })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["settings"]["inviteTtlSecs"], 72 * 3600, "{}", r.body);
    assert_eq!(r.json()["settings"]["resetTtlSecs"], 24 * 3600, "{}", r.body);
    // The box login's settings are the owner's alone.
    let r = post(port, "/cli/skin/access/invites", &agent, json!({ "app": "host", "inviteTtl": "1h" })).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);

    // T5.1: mode on, a new person: App-only, stamped, tied; one mail from
    // the agent's address, no stored copy; the link in the fragment; 72 h
    // from the send.
    let before = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let r = post(port, "/cli/skin/users/invite", &agent, json!({ "username": u3, "app": shop, "email": e3 })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!((v["delivered"].clone(), v["sentTo"].clone(), v["from"].clone()), (json!("sent"), json!(e3), json!(from)), "{v}");
    assert!(!r.body.contains("k2inv_"), "an agent never sees the link: {}", r.body);
    assert_eq!(grants_of(&u3), vec![("app".to_string(), shop.clone())]);
    assert_eq!(creator_of(&u3).as_deref(), Some(sales.id.as_str()));
    assert_eq!(tie_of(&u3).expect("tied").reasons, vec!["created".to_string(), "invite".to_string()]);
    let mails = mails_to(&rec, &e3);
    assert_eq!(mails.len(), 1, "one mail");
    let m = &mails[0];
    assert_eq!(m.from, from);
    assert!(m.ephemeral, "the server destroys its stored copy once it takes the mail");
    assert!(m.subject.contains("invited"), "{}", m.subject);
    assert!(m.text_body.contains(&format!("Set your password: {page}#k2inv_")), "{}", m.text_body);
    assert!(m.text_body.contains("Nobody at") && m.text_body.contains("will ask for your password"), "{}", m.text_body);
    for never in ["k2skn_", "?token="] {
        assert!(!m.text_body.contains(never), "{never} in {}", m.text_body);
    }
    for line in m.text_body.lines().filter(|l| l.to_ascii_lowercase().contains("password:")) {
        assert!(line.starts_with(&format!("Set your password: {page}#k2inv_")), "a password value in {line:?}");
    }
    assert_eq!(outbound_rows(&from).len(), 1, "one audit row");
    let rows = invite_rows(&u3);
    assert_eq!(rows.len(), 1);
    let exp = rows[0].expires_at.expect("the clock started at the send");
    assert!(exp >= before + 72 * 3600 && exp <= before + 72 * 3600 + 30, "72 h from the send: {exp} vs {before}");
    // The agent's own mail reads never carry the token.
    let r = get(port, "/cli/mail/outbox", &agent, &format!("project={}", sales.id)).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(!r.body.contains("k2inv_"), "the outbox never shows the link: {}", r.body);
    // Use it: the password is set; the tie stays (only an owner reset drops it).
    let tok = token_in(&m.text_body);
    let r = reset_with(port, &tok, "my-own-pass-1").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(tie_of(&u3).is_some(), "an invite never drops the tie");
    let r = login(port, json!({ "username": u3, "password": "my-own-pass-1", "app": shop })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = reset_with(port, &tok, "again-pass-12").await;
    assert_eq!((r.status, r.body.as_str()), (401, RESET_FAILED), "single use");

    // T5.8: one invite a minute; after a minute a new one voids the older.
    let u4 = format!("u4{tag}");
    let e4 = format!("{u4}@example.test");
    let r = post(port, "/cli/skin/users/invite", &agent, json!({ "username": u4, "app": shop, "email": e4 })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/skin/users/invite", &agent, json!({ "username": u4, "app": shop })).await;
    assert_eq!((r.status, r.code()), (429, "throttled".to_string()), "{}", r.body);
    skin_db().execute("UPDATE app_user_invites SET created_at = created_at - 120 WHERE principal_id = ?1", [principal_id(&u4)]).unwrap();
    let r = post(port, "/cli/skin/users/invite", &agent, json!({ "username": u4, "app": shop })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let m4 = mails_to(&rec, &e4);
    assert_eq!(m4.len(), 2);
    let (first, second) = (token_in(&m4[0].text_body), token_in(&m4[1].text_body));
    let r = reset_with(port, &first, "first-pass-123").await;
    assert_eq!((r.status, r.body.as_str()), (401, RESET_FAILED), "voided");
    let r = reset_with(port, &second, "second-pass-12").await;
    assert_eq!(r.status, 200, "{}", r.body);

    // T5.6: an existing person without an email.
    let u5 = format!("u5{tag}");
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": u5, "app": shop })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/skin/users/invite", &agent, json!({ "username": u5, "app": shop })).await;
    assert_eq!((r.status, r.code()), (409, "no_email".to_string()), "{}", r.body);

    // T5.5 + review item 10: another workspace's App or person, or the
    // owner's own person on the agent's App: refused, nothing written; an
    // agent can't pick the sender.
    let theirs = format!("th{tag}");
    owner_post(port, "/cli/skin/users", json!({ "username": theirs, "app": desk, "email": format!("{theirs}@example.test") })).await;
    let owners = format!("ow{tag}");
    owner_post(port, "/cli/skin/users", json!({ "username": owners, "app": shop, "email": format!("{owners}@example.test") })).await;
    let sent_before = outbound_rows(&from).len();
    for body in [
        json!({ "username": format!("nx{tag}"), "app": desk, "email": "nx@example.test" }),
        json!({ "username": theirs, "app": shop }),
        json!({ "username": theirs, "app": desk }),
        json!({ "username": owners, "app": shop }),
    ] {
        let r = post(port, "/cli/skin/users/invite", &agent, body.clone()).await;
        assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{body}: {}", r.body);
    }
    assert!(tie_of(&owners).is_none(), "an agent's refused invite ties nobody");
    let r = post(port, "/cli/skin/users/invite", &agent, json!({ "username": u3, "app": shop, "from": "other@example.test" })).await;
    assert_eq!(r.status, 400, "an agent can't pick the sender: {}", r.body);
    assert!(!principal_exists(&format!("nx{tag}")));
    assert!(invite_rows(&theirs).is_empty());
    assert_eq!(outbound_rows(&from).len(), sent_before);

    // TTL and template from the App's settings (Rosson 2026-10-24).
    let r = post(
        port,
        "/cli/skin/access/invites",
        OWNER,
        json!({ "app": shop, "inviteTtl": "15m", "inviteSubject": "Welcome to {app}, {name}", "inviteBody": "Hi {username}, the {app} team ({sender}) saved you a seat." }),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["settings"]["inviteTtlSecs"], 900);
    let u6 = format!("u6{tag}");
    let e6 = format!("{u6}@example.test");
    let r = post(port, "/cli/skin/users/invite", &agent, json!({ "username": u6, "app": shop, "email": e6 })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let m6 = mails_to(&rec, &e6);
    assert_eq!(m6.len(), 1);
    assert_eq!(m6[0].subject, format!("Welcome to shop{tag}, u6{tag}"));
    assert!(m6[0].text_body.starts_with(&format!("Hi u6{tag}, the shop{tag} team (the ")), "{}", m6[0].text_body);
    assert!(m6[0].text_body.contains(&format!("Set your password: {page}#k2inv_")), "K2 adds the link: {}", m6[0].text_body);
    assert!(m6[0].text_body.contains("15 minutes after it was sent"), "{}", m6[0].text_body);
    let row = invite_rows(&u6).remove(0);
    assert_eq!(row.ttl_secs, 900);
    assert!(row.expires_at.unwrap() - row.created_at <= 900 + 30);
    let r = post(port, "/cli/skin/access/invites", OWNER, json!({ "app": shop, "inviteTtl": null, "inviteSubject": null, "inviteBody": null })).await;
    assert_eq!(r.json()["settings"]["inviteTtlSecs"], 72 * 3600, "back to the default: {}", r.body);

    // T5.2: approval: held, the link dead until the owner approves; the
    // approvals preview hides it.
    set_mode(&sales, "approval");
    let u7 = format!("u7{tag}");
    let e7 = format!("{u7}@example.test");
    let r = post(port, "/cli/skin/users/invite", &agent, json!({ "username": u7, "app": shop, "email": e7 })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["delivered"], "held", "{}", r.body);
    assert!(r.json()["hint"].as_str().unwrap().contains("72 hours start when it's sent"), "{}", r.body);
    let out_id = r.json()["outboundId"].as_str().unwrap().to_string();
    assert!(mails_to(&rec, &e7).is_empty(), "nothing left yet");
    let (_, status, body_ref) = outbound_rows(&from).into_iter().find(|(id, _, _)| *id == out_id).expect("the held row");
    assert_eq!(status, "pending");
    let held: Value = serde_json::from_str(&std::fs::read_to_string(&body_ref).unwrap()).unwrap();
    let held_tok = token_in(held["textBody"].as_str().unwrap());
    let r = get(port, "/cli/mail/approvals/list", OWNER, "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(!r.body.contains(&held_tok) && r.body.contains("k2inv_[hidden]"), "the preview hides the link: {}", r.body);
    assert_eq!(invite_rows(&u7)[0].expires_at, None);
    let r = reset_with(port, &held_tok, "early-pass-123").await;
    assert_eq!((r.status, r.body.as_str()), (401, RESET_FAILED), "a held link never works");
    let r = post(port, "/cli/mail/approvals/approve", OWNER, json!({ "id": out_id })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let sent = mails_to(&rec, &e7);
    assert_eq!(sent.len(), 1, "it left on approval");
    assert!(sent[0].ephemeral, "an approved link mail leaves no copy either");
    assert!(invite_rows(&u7)[0].expires_at.is_some(), "the clock started");
    let r = reset_with(port, &held_tok, "later-pass-123").await;
    assert_eq!(r.status, 200, "{}", r.body);

    // T5.3: off: 403 gated, nothing written (no person, no link, no row).
    set_mode(&sales, "off");
    let u8 = format!("u8{tag}");
    let rows_before = outbound_rows(&from).len();
    let r = post(port, "/cli/skin/users/invite", &agent, json!({ "username": u8, "app": shop, "email": format!("{u8}@example.test") })).await;
    assert_eq!((r.status, r.code()), (403, "gated".to_string()), "{}", r.body);
    assert!(!principal_exists(&u8));
    assert_eq!(outbound_rows(&from).len(), rows_before);

    // T5.9: the per-address limits hold (20 an hour).
    set_mode(&sales, "on");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let have = rows_before as i64;
        for i in have..20 {
            conn.execute(
                "INSERT INTO mail_outbound (id, owner_project_id, agent_name, from_address, to_json, subject, body_ref, status, created_at, updated_at)
                 VALUES (?1, ?2, 'x', ?3, '[]', 's', '', 'sent', ?4, ?4)",
                rusqlite::params![format!("out_fill{i}{tag}"), sales.id, from, now],
            )
            .expect("fill");
        }
    }
    let r = post(port, "/cli/skin/users/invite", &agent, json!({ "username": format!("u9{tag}"), "app": shop, "email": "u9@example.test" })).await;
    assert_eq!((r.status, r.code()), (429, "rate_limited".to_string()), "{}", r.body);
    assert!(invite_rows(&format!("u9{tag}")).is_empty(), "a refused send leaves no link");

    // T5.4: no address to send from: an agent is refused and never sees a
    // link; the owner gets the link once. Using it ties the person to the
    // App's workspace (the App's page could see the token).
    let lone = seed_ws(&env, &format!("lone{tag}"));
    let lone_app = app_row(&lone, &format!("lapp{tag}"));
    allow_manage(&lone, true);
    let lone_agent = passport(&lone);
    owner_post(port, "/cli/skin/access/invites", json!({ "app": lone_app, "setPasswordUrl": page })).await;
    let r = post(port, "/cli/skin/users/invite", &lone_agent, json!({ "username": format!("la{tag}"), "app": lone_app, "email": "la@example.test" })).await;
    assert_eq!((r.status, r.code()), (409, "no_mail".to_string()), "{}", r.body);
    assert!(!r.body.contains("k2inv_"), "{}", r.body);
    assert!(!principal_exists(&format!("la{tag}")));
    let lo = format!("lo{tag}");
    let r = post(port, "/cli/skin/users/invite", OWNER, json!({ "username": lo, "app": lone_app, "email": "lo@example.test" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["delivered"], "returned");
    let link = r.json()["link"].as_str().expect("the owner's link").to_string();
    assert!(link.starts_with(&format!("{page}#k2inv_")), "{link}");
    assert!(tie_of(&lo).is_none(), "nobody is tied until the link is used");
    let r = reset_with(port, link.split('#').nth(1).unwrap(), "owner-made-pass-1").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let t = tie_of(&lo).expect("an invite an App page could see ties as it is used");
    assert_eq!((t.workspace_id.as_str(), t.reasons.clone()), (lone.id.as_str(), vec!["invite".to_string()]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s4_the_owners_reset_link_drops_the_tie_and_nothing_else_does() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let rec = recorder();
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let sales = seed_ws(&env, &format!("sales{tag}"));
    let ops = seed_ws(&env, &format!("ops{tag}"));
    let shop = app_row(&sales, &format!("shop{tag}"));
    allow_manage(&sales, true);
    let agent = passport(&sales);
    let domain = format!("rst{tag}.example.test");
    let ws_mailbox = seed_mail(&sales, &domain, "on");
    let system = seed_system_sender(&domain);
    let pw = "agent-chosen-1";
    let role = format!("r{tag}");
    owner_post(port, "/cli/skin/roles", json!({ "name": role, "roomAccess": [{ "handle": sales.handle, "caps": ["thread:read"] }] })).await;

    // u1: the agent made it, set its password and its email.
    let u1 = format!("u1{tag}");
    let e1 = format!("{u1}@example.test");
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": u1, "app": shop, "role": role, "password": pw })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/skin/users/email", &agent, json!({ "username": u1, "email": e1 })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let s_old = login(port, json!({ "username": u1, "password": pw, "app": shop })).await.json()["token"].as_str().unwrap().to_string();

    // No front door yet: K2 has no page of its own; only --code works, and
    // it hands the bare code.
    let r = post(port, "/cli/skin/users/reset-link", OWNER, json!({ "username": u1, "to": "x@example.test", "from": system })).await;
    assert_eq!((r.status, r.code()), (409, "no_front_door".to_string()), "{}", r.body);
    let r = post(port, "/cli/skin/users/reset-link", OWNER, json!({ "username": u1, "code": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.json()["link"].is_null() && r.json()["code"].as_str().is_some_and(|c| c.starts_with("k2inv_")), "{}", r.body);
    let door = format!("https://door{tag}.example.test");
    set_front_door(port, &door).await;
    let k2_page = format!("{door}/cli/skin/password/set");
    // The App's own page and template are never used for an owner reset.
    owner_post(
        port,
        "/cli/skin/access/invites",
        json!({ "app": shop, "setPasswordUrl": format!("{door}/apps/shop/set"), "resetSubject": "App reset {app}", "resetBody": "The App's own reset text.", "resetTtl": "2h" }),
    )
    .await;

    // T4.8: the owner widens: one card (here an owners-only note: the
    // workspace has no Thread); a later widen adds none. It never offers
    // the email the agent set.
    owner_post(port, "/cli/skin/grants/host", json!({ "username": u1, "present": true })).await;
    owner_post(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ops.handle, "caps": ["thread:read"] })).await;
    let card_notes = || -> Vec<k2_core::feedback::FeedbackItem> {
        k2_core::feedback::list_for_project(&sales.id, &k2_core::feedback::ListFilter::Open)
            .unwrap()
            .into_iter()
            .filter(|i| i.title == format!("Extra access for {u1} waits for a password reset"))
            .collect()
    };
    wait_until("the owner's card", 10, || !card_notes().is_empty()).await;
    k2_daemon::app_user_invites::sweep_pending_now();
    let notes = card_notes();
    assert_eq!(notes.len(), 1, "one card per newly pending person");
    let note = notes[0].body.clone().unwrap_or_default();
    assert!(note.contains("the box login"), "{note}");
    assert!(note.contains("could have chosen the stored email") && note.contains("never goes there"), "{note}");
    assert!(note.contains("--code") && !note.contains("k2inv_"), "the note points to the owner's terminal: {note}");
    assert_eq!(user_row(port, &u1).await["storedEmailRefused"], "tied");

    // T4.4: never an agent's (manage-Apps on).
    let r = post(port, "/cli/skin/users/reset-link", &agent, json!({ "username": u1 })).await;
    assert_eq!((r.status, r.code()), (403, "owner_only".to_string()), "{}", r.body);

    // Review items 2 and 3: the agent-set email is refused; mail needs a
    // sender no agent reads; the workspace mailbox is refused.
    let r = post(port, "/cli/skin/users/reset-link", OWNER, json!({ "username": u1, "from": system })).await;
    assert_eq!((r.status, r.code()), (409, "email_unconfirmed".to_string()), "{}", r.body);
    // Verification item 1: any tie refuses the stored email, not just an
    // email the agent set later: here the agent gave it at create.
    let k = format!("k{tag}");
    let ek = format!("{k}@agent-chosen.test");
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": k, "app": shop, "email": ek })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(tie_of(&k).expect("tied").reasons, vec!["created".to_string()]);
    let r = post(port, "/cli/skin/users/reset-link", OWNER, json!({ "username": k, "from": system })).await;
    assert_eq!((r.status, r.code()), (409, "email_unconfirmed".to_string()), "{}", r.body);
    assert!(mails_to(&rec, &ek).is_empty(), "nothing went to the address the agent chose");
    // The owner's --code reset lifts the tie, but the agent's address stays
    // unconfirmed until the owner sets it.
    let r = post(port, "/cli/skin/users/reset-link", OWNER, json!({ "username": k, "code": true })).await;
    let ktok = r.json()["link"].as_str().expect("link").split('#').nth(1).unwrap().to_string();
    assert_eq!(reset_with(port, &ktok, "k-own-pass-123").await.status, 200);
    assert!(tie_of(&k).is_none());
    assert_eq!(user_row(port, &k).await["storedEmailRefused"], "unconfirmed");
    let r = post(port, "/cli/skin/users/reset-link", OWNER, json!({ "username": k, "from": system })).await;
    assert_eq!((r.status, r.code()), (409, "email_unconfirmed".to_string()), "{}", r.body);
    owner_post(port, "/cli/skin/users/email", json!({ "username": k, "email": ek })).await;
    assert_eq!(user_row(port, &k).await["storedEmailRefused"], Value::Null, "the owner set it: confirmed");
    let r = post(port, "/cli/skin/users/reset-link", OWNER, json!({ "username": k, "from": system })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(mails_to(&rec, &ek).len(), 1);
    let r = post(port, "/cli/skin/users/reset-link", OWNER, json!({ "username": u1, "to": "real@example.test" })).await;
    assert_eq!((r.status, r.code()), (409, "owner_sender_required".to_string()), "{}", r.body);
    let r = post(port, "/cli/skin/users/reset-link", OWNER, json!({ "username": u1, "to": "real@example.test", "from": ws_mailbox })).await;
    assert_eq!((r.status, r.code()), (409, "sender_agent_readable".to_string()), "{}", r.body);
    // The system sender is not readable through k2 mail by the agent.
    let r = get(port, "/cli/mail/messages", &agent, &format!("project={}&address={}", sales.id, enc(&system))).await;
    assert_ne!(r.status, 200, "the agent can't read the system sender: {}", r.body);

    // T4.5: none of these drop the tie.
    // - the old forgot → reset;
    let r = post(port, "/cli/skin/password/forgot", OWNER, json!({ "username": u1 })).await;
    let old_tok = r.json()["token"].as_str().expect("owner forgot token").to_string();
    let r = reset_with(port, &old_tok, "forgot-pass-123").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(tie_of(&u1).is_some(), "forgot → reset keeps the tie");
    // - password/change;
    let s = login(port, json!({ "username": u1, "password": "forgot-pass-123", "app": shop })).await.json()["token"].as_str().unwrap().to_string();
    let r = post(port, "/cli/skin/password/change", &s, json!({ "oldPassword": "forgot-pass-123", "password": "changed-pass-12" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(tie_of(&u1).is_some(), "password/change keeps the tie");
    // - an App key's reset: mailed (never returned to the key), tying.
    let (_, key) = k2_core::api_keys::create_app_key("t4", &shop, r#"{"scopes":["users","rooms"]}"#).expect("app key");
    let u2 = format!("u2{tag}");
    let e2 = format!("{u2}@example.test");
    let r = bearer_post(port, &format!("/v1/apps/{shop}/users"), &key, json!({ "username": u2, "email": e2, "password": "app-key-pass-1" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let t2 = tie_of(&u2).expect("an App key's person is tied (Q3)");
    assert_eq!((t2.workspace_id.as_str(), t2.reasons.clone()), (sales.id.as_str(), vec!["app_key".to_string()]));
    assert!(t2.set_by.starts_with("app-key:"), "{t2:?}");
    // An owner reset waiting for u2 is never voided by the App's own reset;
    // while it is live the App's reset waits for it (app-user-profile
    // §5.4): nothing tied anew, minted or mailed.
    let r = post(port, "/cli/skin/users/reset-link", OWNER, json!({ "username": u2, "code": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let owner_tok = r.json()["link"].as_str().unwrap().split('#').nth(1).unwrap().to_string();
    let r = bearer_post(port, &format!("/v1/apps/{shop}/users/password-reset"), &key, json!({ "username": u2 })).await;
    assert_eq!((r.status, r.json()["error"]["code"].as_str()), (409, Some("owner_reset_pending")), "{}", r.body);
    assert!(mails_to(&rec, &e2).is_empty(), "nothing mailed while the owner's reset is live");
    // Once the owner's link has run out, the App's own reset goes again.
    skin_db()
        .execute("UPDATE app_user_invites SET expires_at = 1 WHERE principal_id = ?1 AND purpose = 'owner_reset'", [principal_id(&u2)])
        .unwrap();
    let r = bearer_post(port, &format!("/v1/apps/{shop}/users/password-reset"), &key, json!({ "username": u2 })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["delivered"], "sent", "{}", r.body);
    assert!(!r.body.contains("k2inv_") && r.json()["token"].is_null(), "the key never sees the token: {}", r.body);
    let mk = mails_to(&rec, &e2);
    assert_eq!(mk.len(), 1);
    assert!(mk[0].ephemeral);
    assert!(
        invite_rows(&u2).iter().any(|r| r.purpose == "owner_reset" && r.used_at.is_none()),
        "the App's reset never voids the owner's pending reset (review item 6)"
    );
    let r = reset_with(port, &token_in(&mk[0].text_body), "key-reset-pass-1").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(tie_of(&u2).is_some(), "an App key's reset keeps the tie");

    // T4.1: the owner's reset: K2's own page and fixed text, from the
    // system sender, to another email; using it sets the password, ends
    // every session, drops the tie, stores the new email: full access.
    let r = post(port, "/cli/skin/users/reset-link", OWNER, json!({ "username": u1, "to": "real@example.test", "from": system })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    assert_eq!((v["delivered"].clone(), v["sentTo"].clone(), v["from"].clone()), (json!("sent"), json!("real@example.test"), json!(system)), "{v}");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let exp = v["expiresAt"].as_i64().expect("expiresAt");
    assert!((now + 24 * 3600 - 30..=now + 24 * 3600 + 30).contains(&exp), "24 h, never the App's 2 h (verification item 7): {exp}");
    let m = mails_to(&rec, "real@example.test");
    assert_eq!(m.len(), 1);
    assert!(m[0].ephemeral);
    assert!(m[0].subject.starts_with("Reset your password for"), "K2's text, not the App's: {}", m[0].subject);
    assert!(!m[0].text_body.contains("The App's own reset text"), "{}", m[0].text_body);
    assert!(m[0].text_body.contains(&format!("Set your password: {k2_page}#k2inv_")), "K2's own page: {}", m[0].text_body);
    let tok = token_in(&m[0].text_body);
    let r = reset_with(port, &tok, "their-own-pass-1").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(tie_of(&u1).is_none(), "an owner reset drops the tie");
    assert!(policy_of(&s_old).is_none(), "every old session ended");
    assert!(policy_of(&s).is_none(), "every old session ended");
    assert_eq!(user_row(port, &u1).await["email"], "real@example.test");
    let r = login(port, json!({ "username": u1, "password": "their-own-pass-1" })).await;
    assert_eq!(r.status, 200, "the box login works now: {}", r.body);
    let r = login(port, json!({ "username": u1, "password": "their-own-pass-1", "app": shop })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let mut both = vec![sales.id.clone(), ops.id.clone()];
    both.sort();
    assert_eq!(rooms(&policy_of(r.json()["token"].as_str().unwrap()).unwrap()), both, "the widened access is live");
    assert_eq!(user_row(port, &u1).await["tiedTo"], Value::Null);

    // T4.2 (verification item 1): u2's email came from the App key, so
    // the stored email is refused; the owner gives the same address
    // (--to), which confirms it on use; then the stored email works.
    let r = post(port, "/cli/skin/users/reset-link", OWNER, json!({ "username": u2, "from": system })).await;
    assert_eq!((r.status, r.code()), (409, "email_unconfirmed".to_string()), "{}", r.body);
    let r = post(port, "/cli/skin/users/reset-link", OWNER, json!({ "username": u2, "to": e2, "from": system })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let m = mails_to(&rec, &e2);
    assert_eq!(m.len(), 2);
    let r = reset_with(port, &token_in(&m[1].text_body), "fresh-pass-1234").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(tie_of(&u2).is_none());
    let r = reset_with(port, &owner_tok, "late-pass-12345").await;
    assert_eq!((r.status, r.body.as_str()), (401, RESET_FAILED), "the newer owner reset voided the older one");
    assert_eq!(user_row(port, &u2).await["storedEmailRefused"], Value::Null);
    let r = post(port, "/cli/skin/users/reset-link", OWNER, json!({ "username": u2, "from": system })).await;
    assert_eq!(r.status, 200, "the owner confirmed it: {}", r.body);
    assert_eq!(mails_to(&rec, &e2).len(), 3);

    // T4.3: --code: nothing mailed, K2's own page to the owner; it works.
    let g = format!("g{tag}");
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": g, "app": shop, "password": pw })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let sent_before = rec.0.lock().unwrap().len();
    let r = post(port, "/cli/skin/users/reset-link", OWNER, json!({ "username": g, "code": true })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["delivered"], "returned");
    assert_eq!(rec.0.lock().unwrap().len(), sent_before, "nothing mailed");
    let link = r.json()["link"].as_str().unwrap().to_string();
    assert!(link.starts_with(&format!("{k2_page}#k2inv_")), "{link}");

    // T4.6: two uses at once: exactly one wins.
    let token = link.split('#').nth(1).unwrap().to_string();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|i| {
            let (t, b) = (token.clone(), barrier.clone());
            std::thread::spawn(move || {
                b.wait();
                k2_core::skin::invites::consume(&t, &format!("racer-pass-{i}")).expect("consume")
            })
        })
        .collect();
    let outcomes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(outcomes.iter().filter(|o| **o == k2_core::skin::invites::Consume::Ok).count(), 1, "{outcomes:?}");
    assert!(tie_of(&g).is_none());

    // T4.7: an expired link: the generic 401.
    let h = format!("h{tag}");
    let r = post(port, "/cli/skin/users", &agent, json!({ "username": h, "app": shop })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = post(port, "/cli/skin/users/reset-link", OWNER, json!({ "username": h, "code": true })).await;
    let token = r.json()["link"].as_str().unwrap().split('#').nth(1).unwrap().to_string();
    skin_db().execute("UPDATE app_user_invites SET expires_at = 1 WHERE principal_id = ?1", [principal_id(&h)]).unwrap();
    let r = reset_with(port, &token, "too-late-pass-1").await;
    assert_eq!((r.status, r.body.as_str()), (401, RESET_FAILED));
    assert!(tie_of(&h).is_some());
}

/// Review item 7: an App's forgot answers the same for everyone (one
/// throttle per person across every pass, taken first), never returns a token, and a
/// used App reset link ties the person to the App's workspace.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s0_an_apps_forgot_is_uniform_and_its_link_ties() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let rec = recorder();
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let sales = seed_ws(&env, &format!("sales{tag}"));
    let shop = app_row(&sales, &format!("shop{tag}"));
    let domain = format!("fgt{tag}.example.test");
    seed_mail(&sales, &domain, "on");
    let door = format!("https://door{tag}.example.test");
    set_front_door(port, &door).await;
    // The owner's guest of the App, wholly inside the workspace.
    let g = format!("g{tag}");
    let e = format!("{g}@example.test");
    owner_post(port, "/cli/skin/users", json!({ "username": g, "app": shop, "email": e, "password": "s3cret-horse" })).await;
    owner_post(port, "/cli/skin/users/rooms", json!({ "username": g, "app": shop, "rooms": [sales.handle] })).await;
    let r = post(port, "/cli/skin-tokens", OWNER, json!({ "name": format!("p{tag}"), "caps": ["thread:read"], "rooms": [sales.handle] })).await;
    let pass = r.json()["token"].as_str().unwrap().to_string();
    let r = post(port, "/cli/skin-tokens", OWNER, json!({ "name": format!("q{tag}"), "caps": ["thread:read"], "rooms": [sales.handle] })).await;
    let pass2 = r.json()["token"].as_str().unwrap().to_string();
    for who in [g.clone(), format!("nobody{tag}")] {
        let r = post(port, "/cli/skin/password/forgot", &pass, json!({ "username": who })).await;
        assert_eq!((r.status, r.json()), (200, json!({ "ok": true })), "{who}: {}", r.body);
        let r = post(port, "/cli/skin/password/forgot", &pass, json!({ "username": who })).await;
        assert_eq!(r.status, 429, "every name is throttled the same way ({who}): {}", r.body);
        // Verification item 6: per person, not per pass.
        let r = post(port, "/cli/skin/password/forgot", &pass2, json!({ "username": who })).await;
        assert_eq!(r.status, 429, "another pass shares the person's clock ({who}): {}", r.body);
    }
    // The owner's own forgot is not blocked by the pass's throttle.
    let r = post(port, "/cli/skin/password/forgot", OWNER, json!({ "username": g })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    wait_until("the App's reset mail", 10, || !mails_to(&rec, &e).is_empty()).await;
    let m = mails_to(&rec, &e);
    assert!(m[0].ephemeral);
    assert!(tie_of(&g).is_none(), "nothing ties until the link is used");
    let r = reset_with(port, &token_in(&m[0].text_body), "app-reset-pass-1").await;
    assert_eq!(r.status, 200, "{}", r.body);
    let t = tie_of(&g).expect("a used App reset link ties");
    assert_eq!((t.workspace_id.as_str(), t.reasons.clone()), (sales.id.as_str(), vec!["app_reset".to_string()]));
}
