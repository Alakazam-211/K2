//! Access changes reach open sockets (`prd-live-access-changes-v1` S1a).
//!
//! Real daemon, real routes, real sockets, headless. Every socket, every kind
//! of loss: the frame stops and the socket closes. Beside each loss, a person
//! whose access did NOT change stays connected and keeps getting frames.

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;
use serde_json::{json, Value};

const PW: &str = "s3cret-horse-staple";

async fn owner_ok(port: u16, path: &str, body: Value) -> Value {
    let r = post(port, path, OWNER, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

const ALL_CAPS: &[&str] = &["thread:read", "thread:post", "activity:read", "tickets:read", "files:read", "room:members", "heartbeats:read"];

/// One guest on its own role in `ws`. Returns `(role, pass)`.
async fn guest_in(port: u16, ws: &Ws, username: &str, caps: &[&str]) -> (String, String) {
    let role = format!("role-{username}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": role })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": caps })).await;
    owner_ok(port, "/cli/skin/users", json!({ "username": username })).await;
    k2_daemon::test_harness::guest_with_own_password(username, PW);
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": username, "role": role })).await;
    let body = json!({ "username": username, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.expect("join");
    assert_eq!(r.status, 200, "guest login: {}", r.body);
    (role, r.json()["token"].as_str().expect("token").to_string())
}

// ── App uninstall ────────────────────────────────────────────────────

/// `k2 app uninstall --drop-access` says it drops access: the App's login
/// sessions end. Another App's session, and a box login, are untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn uninstalling_an_app_ends_its_guests_sessions_and_no_one_elses() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "uninst");
    let (role, box_login) = guest_in(port, &ws, "ulla", ALL_CAPS).await;
    let (other_role, _) = guest_in(port, &ws, "ugo", ALL_CAPS).await;

    let new_app = |app: &str| {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let svc = k2_core::published_services::insert(&conn, &ws.id, app, k2_core::published_services::CMD_APP_SENTINEL, &ws.path.to_string_lossy(), 0, "local", "stopped", "cmd", "")
            .expect("service")
            .id;
        k2_core::deploy::store::insert_app(
            &conn,
            &k2_core::deploy::store::NewApp {
                app_id: app,
                service_id: &svc,
                seed_id: None,
                owner_workspace: &ws.id,
                isolation: "process",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: "owner",
            },
        )
        .expect("insert app");
        svc
    };
    let grant = |user: &str, role: &str, svc: &str| {
        let pid = k2_core::skin::host_login_detail(user).expect("guest").principal_id;
        k2_core::skin::create_grant(&k2_core::skin::SkinGrantWrite {
            subject_kind: "principal".into(),
            subject_id: pid,
            kind: "app".into(),
            target_id: svc.to_string(),
            role: Some(role.to_string()),
            scope: None,
            created_by: Some("owner".into()),
        })
        .expect("app grant");
        k2_core::skin::create_app_session_token(user, svc).expect("App session").raw
    };
    let gone_svc = new_app("uninstgone");
    let kept_svc = new_app("uninstkept");
    let gone_session = grant("ulla", &role, &gone_svc);
    let kept_session = grant("ugo", &other_role, &kept_svc);
    for s in [&gone_session, &kept_session, &box_login] {
        let r = get(port, "/cli/skin/me", s, "x=1").await;
        assert_eq!(r.status, 200, "before: {}", r.body);
    }

    owner_ok(port, "/cli/app/uninstall", json!({ "app": "uninstgone", "dropAccess": true })).await;

    let after = get(port, "/cli/skin/me", &gone_session, "x=1").await;
    assert_eq!(after.status, 401, "the removed App's session must end: {}", after.body);
    let thread = get(port, "/cli/thread", &gone_session, &format!("addr={}", ws.handle)).await;
    assert_eq!(thread.status, 401, "and every door with it: {}", thread.body);
    // Control: nobody else's access changed.
    for (who, s) in [("the other App's session", &kept_session), ("the same person's box login", &box_login)] {
        let r = get(port, "/cli/skin/me", s, "x=1").await;
        assert_eq!(r.status, 200, "{who} keeps working: {}", r.body);
    }
}
