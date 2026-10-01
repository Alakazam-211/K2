//! Viewer role removal + route policy table — end-to-end through the REAL
//! dispatcher (`k2_daemon::test_harness`), `prd-remove-viewer-role-v1.md` T1.
//!
//!   1. A store written before the removal (`"role":"viewer"`) is migrated
//!      the way boot does it: the account is a DISABLED Member flagged
//!      `wasViewer`, one `role-removed` audit line exists, and it cannot
//!      log in. Enabling it is the explicit "Enable as Member".
//!   2. Creating a viewer is refused with 400 (users/add `role`,
//!      users/set-role) and nothing changes.
//!   3. A Member still reaches member routes (reads, a write, project chat).
//!   4. Admin-only and Owner-only routes still 403 for a Member — now with
//!      `role_required`, never the re-login text — and an Admin passes the
//!      Admin ones.
//!   5. The owner token is unaffected (no role gate, unknown paths keep the
//!      catch-all 404); a login on an unclassified path is refused.
//!
//! Fail loudly: every assertion names what it saw; no defaults.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::connect_users::{self, Role};
use k2_daemon::test_harness;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-remove-viewer-role";

struct Resp {
    status: u16,
    body: String,
}

impl Resp {
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|e| panic!("body is not JSON ({e}): {:?}", self.body))
    }
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .expect("set read timeout");
    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{b}",
            b.len()
        ),
        None => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
        ),
    };
    stream.write_all(req.as_bytes()).expect("write request");
    let mut raw = Vec::new();
    match stream.read_to_end(&mut raw) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        Err(e) => panic!("read response: {e:?}"),
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("could not parse status from response: {text:?}"));
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_else(|| panic!("no header/body split: {text:?}"));
    Resp { status, body }
}

fn with_temp_home<F: FnOnce()>(f: F) {
    let prev = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!("k2-rm-viewer-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(tmp.join(".k2")).expect("create temp HOME/.k2");
    std::env::set_var("HOME", &tmp);
    f();
    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn seed_session(username: &str, role: Role) -> String {
    connect_users::add_user(username, "password123").expect("add_user");
    if role != Role::Member {
        connect_users::set_role(username, role).expect("set_role");
    }
    connect_users::create_session(username)
}

fn login(port: u16, username: &str, password: &str) -> Resp {
    http(
        port,
        "POST",
        "/cli/auth/login",
        Some(&serde_json::json!({ "username": username, "password": password }).to_string()),
    )
}

fn users_row(port: u16, username: &str) -> serde_json::Value {
    let r = http(port, "GET", &format!("/cli/users?token={OWNER_TOKEN}"), None);
    assert_eq!(r.status, 200, "owner GET /cli/users: {}", r.body);
    let v = r.json();
    let users = v["users"]
        .as_array()
        .unwrap_or_else(|| panic!("users array missing: {v}"));
    users
        .iter()
        .find(|u| u["username"] == username)
        .unwrap_or_else(|| panic!("{username} missing from /cli/users: {v}"))
        .clone()
}

fn assert_role_required(r: &Resp, required: &str, what: &str) {
    assert_eq!(r.status, 403, "{what}: {}", r.body);
    let v = r.json();
    assert_eq!(v["error"], "role_required", "{what}: {v}");
    assert_eq!(v["required"], required, "{what}: {v}");
    assert!(
        !r.body.contains("invalid or missing token"),
        "{what}: a role refusal must not look like a dead login: {}",
        r.body
    );
}

// ─────────────────────────────────────────────────────────────────────
// 1 — stored viewer: disabled on update, audited, can't log in
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stored_viewer_is_disabled_audited_and_cannot_log_in() {
    let _g = lock();
    with_temp_home(|| {
        // A store as a pre-removal daemon wrote it.
        connect_users::add_user("mel", "password123").expect("add mel");
        let store = connect_users::store_path();
        let raw = std::fs::read_to_string(&store).expect("read store");
        let mut v: serde_json::Value = serde_json::from_str(&raw).expect("store json");
        let mel = v["users"][0].clone();
        let mut vera = mel.clone();
        vera["username"] = serde_json::json!("vera");
        vera["role"] = serde_json::json!("viewer");
        vera["disabled"] = serde_json::json!(false);
        v["users"] = serde_json::json!([mel, vera]);
        std::fs::write(&store, v.to_string()).expect("write legacy store");

        // What `main.rs` runs at boot.
        let migrated = connect_users::migrate_removed_viewers().expect("boot migration");
        assert_eq!(migrated, vec!["vera".to_string()]);

        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // Settings sees a disabled Member marked wasViewer.
        let row = users_row(d.port, "vera");
        assert_eq!(row["role"], "member", "row: {row}");
        assert_eq!(row["disabled"], true, "row: {row}");
        assert_eq!(row["wasViewer"], true, "row: {row}");
        let mel_row = users_row(d.port, "mel");
        assert_eq!(mel_row["wasViewer"], false, "row: {mel_row}");
        assert_eq!(mel_row["disabled"], false, "row: {mel_row}");

        // One audit line, readable through the daemon.
        let r = http(d.port, "GET", &format!("/cli/users/audit?tail=50&token={OWNER_TOKEN}"), None);
        assert_eq!(r.status, 200, "audit: {}", r.body);
        let events = r.json()["events"].clone();
        let lines: Vec<&serde_json::Value> = events
            .as_array()
            .unwrap_or_else(|| panic!("events array missing: {events}"))
            .iter()
            .filter(|e| e["event"] == "role-removed")
            .collect();
        assert_eq!(lines.len(), 1, "audit events: {events}");
        assert_eq!(lines[0]["user"], "vera");
        assert_eq!(lines[0]["outcome"], "viewer-disabled");

        // The former viewer cannot log in; the member can.
        let r = login(d.port, "vera", "password123");
        assert_eq!(r.status, 401, "former viewer must not log in: {}", r.body);
        let r = login(d.port, "mel", "password123");
        assert_eq!(r.status, 200, "member login: {}", r.body);

        // "Enable as Member" = enable; the flag clears and login works.
        let r = http(
            d.port,
            "POST",
            &format!("/cli/users/set-disabled?token={OWNER_TOKEN}"),
            Some(r#"{"username":"vera","disabled":false}"#),
        );
        assert_eq!(r.status, 200, "enable: {}", r.body);
        let row = users_row(d.port, "vera");
        assert_eq!(row["disabled"], false, "row: {row}");
        assert_eq!(row["wasViewer"], false, "row: {row}");
        assert_eq!(row["role"], "member", "row: {row}");
        let r = login(d.port, "vera", "password123");
        assert_eq!(r.status, 200, "enabled former viewer logs in as member: {}", r.body);
        let tok = r.json()["token"]
            .as_str()
            .unwrap_or_else(|| panic!("login token missing: {}", r.body))
            .to_string();
        let r = http(d.port, "GET", &format!("/cli/auth/whoami?token={tok}"), None);
        assert_eq!(r.status, 200, "whoami: {}", r.body);
        assert_eq!(r.json()["role"], "member", "whoami: {}", r.body);
    });
}

// ─────────────────────────────────────────────────────────────────────
// 2 — creating a viewer is a 400 and changes nothing
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn creating_a_viewer_returns_400() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // users/add with role viewer: refused, nobody created.
        let r = http(
            d.port,
            "POST",
            &format!("/cli/users/add?token={OWNER_TOKEN}"),
            Some(r#"{"username":"newview","password":"password123","role":"viewer"}"#),
        );
        assert_eq!(r.status, 400, "users/add role viewer: {}", r.body);
        assert_eq!(r.json()["error"], connect_users::VIEWER_ROLE_REMOVED);
        assert_eq!(connect_users::role_for_user("newview"), None, "nobody created");

        // users/set-role viewer on an existing member: refused, role kept.
        connect_users::add_user("staysmember", "password123").expect("add");
        let r = http(
            d.port,
            "POST",
            &format!("/cli/users/set-role?token={OWNER_TOKEN}"),
            Some(r#"{"username":"staysmember","role":"viewer"}"#),
        );
        assert_eq!(r.status, 400, "set-role viewer: {}", r.body);
        assert_eq!(r.json()["error"], connect_users::VIEWER_ROLE_REMOVED);
        assert_eq!(connect_users::role_for_user("staysmember"), Some(Role::Member));

        // users/add role member still works.
        let r = http(
            d.port,
            "POST",
            &format!("/cli/users/add?token={OWNER_TOKEN}"),
            Some(r#"{"username":"plainmember","password":"password123","role":"member"}"#),
        );
        assert_eq!(r.status, 200, "users/add role member: {}", r.body);
        assert_eq!(connect_users::role_for_user("plainmember"), Some(Role::Member));
    });
}

// ─────────────────────────────────────────────────────────────────────
// 3 + 4 — members keep member routes; admin/owner routes still refuse them
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_routes_pass_and_admin_owner_routes_refuse_members() {
    let _g = lock();
    with_temp_home(|| {
        let _db = k2_core::db::init_for_tests();
        let member = seed_session("rv_member", Role::Member);
        let admin = seed_session("rv_admin", Role::Admin);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // Member reads.
        for path in ["/cli/projects/list", "/cli/presence/roster", "/cli/auth/whoami"] {
            let r = http(d.port, "GET", &format!("{path}?token={member}"), None);
            assert_eq!(r.status, 200, "member GET {path}: {}", r.body);
        }
        // A member write: create a project group, then it lists.
        let group = format!("rv-group-{}", std::process::id());
        let r = http(
            d.port,
            "POST",
            &format!("/cli/project-group/create?token={member}"),
            Some(&serde_json::json!({ "name": group }).to_string()),
        );
        assert_eq!(r.status, 200, "member project-group/create: {}", r.body);
        let r = http(d.port, "GET", &format!("/cli/project-group/list?token={member}"), None);
        assert_eq!(r.status, 200, "member project-group/list: {}", r.body);
        assert!(r.body.contains(&group), "created group must list: {}", r.body);

        // Admin-floor routes: Member 403 role_required, state unchanged.
        let r = http(d.port, "GET", &format!("/cli/users?token={member}"), None);
        assert_role_required(&r, "admin", "member GET /cli/users");
        let r = http(
            d.port,
            "POST",
            &format!("/cli/users/add?token={member}"),
            Some(r#"{"username":"sneaky","password":"password123"}"#),
        );
        assert_role_required(&r, "admin", "member POST users/add");
        assert_eq!(connect_users::role_for_user("sneaky"), None, "nobody created");
        let r = http(d.port, "POST", &format!("/cli/daemon/restart?token={member}"), Some("{}"));
        assert_role_required(&r, "admin", "member POST daemon/restart");
        let r = http(
            d.port,
            "POST",
            &format!("/cli/presets/create?token={member}"),
            Some(r#"{"label":"x","command":"echo"}"#),
        );
        assert_role_required(&r, "admin", "member POST presets/create");

        // Owner-floor route: Member and Admin both refused.
        for (who, tok) in [("member", &member), ("admin", &admin)] {
            let r = http(
                d.port,
                "POST",
                &format!("/cli/users/set-role?token={tok}"),
                Some(r#"{"username":"rv_member","role":"admin"}"#),
            );
            assert_role_required(&r, "owner", &format!("{who} POST users/set-role"));
        }
        assert_eq!(connect_users::role_for_user("rv_member"), Some(Role::Member));

        // Owner-token-only route: refused for every login.
        let r = http(d.port, "POST", &format!("/cli/tunnel/stop?token={admin}"), Some("{}"));
        assert_role_required(&r, "owner-token", "admin POST tunnel/stop");

        // Admin passes the Admin floor, exactly as before.
        let r = http(d.port, "GET", &format!("/cli/users?token={admin}"), None);
        assert_eq!(r.status, 200, "admin GET /cli/users: {}", r.body);
        let r = http(d.port, "POST", &format!("/cli/daemon/restart?token={admin}"), Some("{}"));
        assert_eq!(r.status, 200, "admin daemon/restart (harness never restarts): {}", r.body);
    });
}

// ─────────────────────────────────────────────────────────────────────
// 5 — owner token unaffected; unclassified paths refused for logins
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_token_is_unaffected_and_unclassified_is_refused_for_logins() {
    let _g = lock();
    with_temp_home(|| {
        let _first = seed_session("rv_member2", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        // Owner token on admin/owner routes: 200 as always.
        let r = http(d.port, "GET", &format!("/cli/users?token={OWNER_TOKEN}"), None);
        assert_eq!(r.status, 200, "owner GET /cli/users: {}", r.body);
        let r = http(
            d.port,
            "POST",
            &format!("/cli/users/set-role?token={OWNER_TOKEN}"),
            Some(r#"{"username":"rv_member2","role":"admin"}"#),
        );
        assert_eq!(r.status, 200, "owner set-role: {}", r.body);
        assert_eq!(connect_users::role_for_user("rv_member2"), Some(Role::Admin));

        // Unknown path: the owner token keeps the catch-all 404 ...
        let r = http(d.port, "GET", &format!("/cli/no-such-route?token={OWNER_TOKEN}"), None);
        assert_eq!(r.status, 404, "owner unknown path: {}", r.body);
        assert_eq!(r.json()["error"], "route not found", "owner unknown path: {}", r.body);

        // ... and a login is refused by the policy table. (set-role revoked
        // the old session; mint a fresh one.)
        let fresh = connect_users::create_session("rv_member2");
        let r = http(d.port, "GET", &format!("/cli/no-such-route?token={fresh}"), None);
        assert_eq!(r.status, 404, "login unknown path: {}", r.body);
        assert_eq!(r.json()["error"], "route_unclassified", "login unknown path: {}", r.body);

        // The Viewer grant route is gone for everyone.
        let r = http(
            d.port,
            "POST",
            &format!("/cli/presence/grant?token={OWNER_TOKEN}"),
            Some(r#"{"username":"rv_member2","granted":true}"#),
        );
        assert_eq!(r.status, 405, "presence/grant gone: {}", r.body);

        // No token / a garbage token still get the per-route 403.
        let r = http(d.port, "GET", "/cli/users?token=garbage", None);
        assert_eq!(r.status, 403, "garbage token: {}", r.body);
    });
}
