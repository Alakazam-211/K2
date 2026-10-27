//! Thread push notifications (prd-thread-push-notifications-v1), end to
//! end through the REAL dispatcher (`test_harness`, in-process, ephemeral
//! port, temp `$HOME`). Raw HTTP / WS over loopback. Covers:
//!
//! 1. TP4 security fix: a Member login reading or echoing settings never
//!    sees the fleet push gateway token; the owner token does.
//! 2. `/cli/notify/*` gates: GET on the POST-only path is 405; a Member
//!    gets 403 `role_required` on the admin paths; an agent passport gets
//!    403 on all four; identity is the token's.
//! 3. Owner-linked accounts: only the owner token may set them (logins of
//!    any role and passports are refused); a linked login and the owner
//!    token share one prefs row.
//! 4. S4: an overlay WS `viewing` frame from the owner token marks the
//!    owner as viewing that Thread (state, 90 s TTL), and closing the
//!    socket clears it.
//!
//! No push ever leaves: no gateway is configured and no tunnel is paired.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use futures_util::SinkExt;
use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use rusqlite::params;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "owner-token-thread-push-0455";

struct Resp {
    status: u16,
    body: String,
}

impl Resp {
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("JSON ({e}): {:?}", self.body))
    }
}

fn req(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(15))).expect("timeout");
    let raw_req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None if method == "POST" => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
        ),
        None => format!("{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
    };
    stream.write_all(raw_req.as_bytes()).expect("write");
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof) => break,
            Err(e) => panic!("{method} {path_and_query}: read: {e:?}"),
        }
    }
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no status in {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

fn with_temp_home<F: FnOnce()>(f: F) {
    struct Restore(Option<std::ffi::OsString>, std::path::PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            match self.0.take() {
                Some(p) => std::env::set_var("HOME", p),
                None => std::env::remove_var("HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.1);
        }
    }
    let tmp = std::env::temp_dir().join(format!("k2-thread-push-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(tmp.join(".k2")).expect("temp HOME");
    let _restore = Restore(std::env::var_os("HOME"), tmp.clone());
    std::env::set_var("HOME", &tmp);
    f();
}

fn login(port: u16, username: &str, role: &str) -> String {
    let r = req(
        port,
        "POST",
        &format!("/cli/users/add?token={OWNER_TOKEN}"),
        Some(&format!(r#"{{"username":"{username}","password":"password123"}}"#)),
    );
    assert_eq!(r.status, 200, "users/add {username}: {}", r.body);
    if role != "member" {
        let r = req(
            port,
            "POST",
            &format!("/cli/users/set-role?token={OWNER_TOKEN}"),
            Some(&format!(r#"{{"username":"{username}","role":"{role}"}}"#)),
        );
        assert_eq!(r.status, 200, "set-role: {}", r.body);
    }
    let r = req(port, "POST", "/cli/auth/login", Some(&format!(r#"{{"username":"{username}","password":"password123"}}"#)));
    assert_eq!(r.status, 200, "login {username}: {}", r.body);
    r.json()["token"].as_str().expect("token").to_string()
}

fn seed_ws(handle: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
        params![id, handle, format!("/tmp/tp-int-{handle}-{id}")],
    )
    .expect("seed project");
    id
}

fn passport(workspace_uuid: &str) -> String {
    let sid = SessionId::new();
    k2_daemon::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        HookPrincipal { workspace_uuid: workspace_uuid.to_string(), agent_address: "tp-agent".to_string() },
        CredMode::ApiKey,
        Provider::Anthropic,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_hide_the_push_gateway_token_from_members() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let r = req(
            d.port,
            "POST",
            &format!("/cli/settings/update?token={OWNER_TOKEN}"),
            Some(r#"{"pushGatewayToken":"fleet-secret-xyz","pushGatewayUrl":"https://u:p@push.example"}"#),
        );
        assert_eq!(r.status, 200, "{}", r.body);
        assert!(r.body.contains("fleet-secret-xyz"), "the owner token's own echo keeps it");
        let member = login(d.port, "tpmember", "member");
        let owner_role = login(d.port, "tpowner", "owner");
        for tok in [&member, &owner_role] {
            let r = req(d.port, "GET", &format!("/cli/settings/get?token={tok}"), None);
            assert_eq!(r.status, 200, "{}", r.body);
            assert!(!r.body.contains("fleet-secret-xyz"), "a login read the fleet token: {}", r.body);
            assert!(!r.body.contains("u:p@"), "URL userinfo leaked: {}", r.body);
            assert!(r.json()["pushGatewayToken"].is_null());
        }
        let r = req(d.port, "POST", &format!("/cli/settings/update?token={member}"), Some(r#"{"defaultAgent":"codex"}"#));
        assert_eq!(r.status, 200, "{}", r.body);
        assert!(!r.body.contains("fleet-secret-xyz"), "the update echo leaked it: {}", r.body);
        let r = req(d.port, "GET", &format!("/cli/settings/get?token={OWNER_TOKEN}"), None);
        assert_eq!(r.json()["pushGatewayToken"], "fleet-secret-xyz", "the owner token still sees it");
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notify_routes_gate_by_role_and_refuse_passports() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let ws = seed_ws(&format!("tpws{}", &uuid::Uuid::new_v4().to_string()[..6]));
        let member = login(d.port, "tpmo", "member");
        let admin = login(d.port, "tpz3", "admin");
        let agent = passport(&ws);

        // GET on the POST-only path: 405, for every caller.
        for tok in [OWNER_TOKEN, member.as_str()] {
            let r = req(d.port, "GET", &format!("/cli/notify/admin/set?token={tok}"), None);
            assert_eq!(r.status, 405, "{}", r.body);
        }
        // Member: own row yes, admin paths role_required.
        let r = req(d.port, "GET", &format!("/cli/notify/prefs?token={member}"), None);
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.json()["principal"], "tpmo");
        for (m, p) in [("GET", "/cli/notify/admin"), ("POST", "/cli/notify/admin/set")] {
            let r = req(d.port, m, &format!("{p}?token={member}"), if m == "POST" { Some("{}") } else { None });
            assert_eq!(r.status, 403, "{m} {p}: {}", r.body);
            assert!(r.body.contains("role_required"), "{}", r.body);
        }
        // An agent passport: 403 on all four (and on the owner links).
        for (m, p) in [
            ("GET", "/cli/notify/prefs"),
            ("POST", "/cli/notify/prefs"),
            ("GET", "/cli/notify/admin"),
            ("POST", "/cli/notify/admin/set"),
            ("GET", "/cli/owner/accounts"),
            ("POST", "/cli/owner/accounts"),
        ] {
            let body = if m == "POST" { Some(r#"{"server":{"enabled":false},"accounts":["tpz3"]}"#) } else { None };
            let r = req(d.port, m, &format!("{p}?token={agent}"), body);
            assert_eq!(r.status, 403, "passport {m} {p}: {}", r.body);
        }
        // Identity is the token's: a body principal is ignored.
        let r = req(
            d.port,
            "POST",
            &format!("/cli/notify/prefs?token={member}"),
            Some(&format!(r#"{{"principal":"tpz3","workspace":{{"projectId":"{ws}","threadPush":false}}}}"#)),
        );
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.json()["principal"], "tpmo");
        // Unknown projectId → 400 usage.
        let r = req(
            d.port,
            "POST",
            &format!("/cli/notify/prefs?token={member}"),
            Some(r#"{"workspace":{"projectId":"nope","threadPush":true}}"#),
        );
        assert_eq!(r.status, 400, "{}", r.body);
        assert_eq!(r.json()["error"]["code"], "usage");
        // An Admin reads everyone but may not change another person's row.
        let r = req(d.port, "GET", &format!("/cli/notify/admin?token={admin}"), None);
        assert_eq!(r.status, 200, "{}", r.body);
        let r = req(
            d.port,
            "POST",
            &format!("/cli/notify/admin/set?token={admin}"),
            Some(r#"{"principal":"tpmo","server":{"enabled":false}}"#),
        );
        assert_eq!(r.status, 403, "{}", r.body);
        assert!(r.body.contains("owner_only"), "{}", r.body);

        // Owner-linked accounts: no login may set them, not even the
        // Admin being linked; the owner token can.
        let r = req(d.port, "POST", &format!("/cli/owner/accounts?token={admin}"), Some(r#"{"accounts":["tpz3"]}"#));
        assert_eq!(r.status, 403, "{}", r.body);
        let r = req(d.port, "POST", &format!("/cli/owner/accounts?token={OWNER_TOKEN}"), Some(r#"{"accounts":["tpz3"]}"#));
        assert_eq!(r.status, 200, "{}", r.body);
        let r = req(d.port, "GET", &format!("/cli/owner/accounts?token={admin}"), None);
        assert_eq!(r.json()["accounts"], serde_json::json!(["tpz3"]));
        // The linked phone login and the desktop owner token: one row.
        let r = req(
            d.port,
            "POST",
            &format!("/cli/notify/prefs?token={admin}"),
            Some(r#"{"server":{"enabled":false}}"#),
        );
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.json()["principal"], "owner");
        let r = req(d.port, "GET", &format!("/cli/notify/prefs?token={OWNER_TOKEN}"), None);
        assert_eq!(r.json()["server"]["enabled"], false, "the desktop sees the phone's bell");
        assert_eq!(r.json()["aliases"], serde_json::json!(["tpz3"]));
        // …and the link grants no owner power: still an Admin.
        let r = req(
            d.port,
            "POST",
            &format!("/cli/notify/admin/set?token={admin}"),
            Some(r#"{"principal":"tpmo","server":{"enabled":false}}"#),
        );
        assert_eq!(r.status, 403, "{}", r.body);
        let r = req(d.port, "POST", &format!("/cli/settings/reset?token={admin}"), None);
        assert_eq!(r.status, 403, "a linked Admin still can't do Owner things: {}", r.body);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_viewing_frame_marks_the_person_viewing_until_the_socket_closes() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let conv = uuid::Uuid::new_v4().to_string();
        let url = format!("ws://127.0.0.1:{}/cli/overlay/events?conversation={conv}&token={OWNER_TOKEN}", d.port);
        let viewers = k2_daemon::thread_viewers::global();
        let wait_for = |want: bool| {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while viewers.is_viewing("owner", &conv, std::time::Instant::now()) != want {
                assert!(std::time::Instant::now() < deadline, "viewing never became {want}");
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        futures_block(async {
            let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.expect("overlay WS opens");
            ws.send(tokio_tungstenite::tungstenite::Message::Text(r#"{"t":"viewing","visible":true}"#.into()))
                .await
                .expect("send viewing");
            tokio::task::block_in_place(|| wait_for(true));
            assert!(!viewers.is_viewing("owner", "another-thread", std::time::Instant::now()));
            ws.send(tokio_tungstenite::tungstenite::Message::Text(r#"{"t":"viewing","visible":false}"#.into()))
                .await
                .expect("send hidden");
            tokio::task::block_in_place(|| wait_for(false));
            ws.send(tokio_tungstenite::tungstenite::Message::Text(r#"{"t":"viewing","visible":true}"#.into()))
                .await
                .expect("send viewing again");
            tokio::task::block_in_place(|| wait_for(true));
            ws.close(None).await.expect("close");
        });
        wait_for(false);
    });
}

/// 0.46.1 push previews, through the real dispatcher: `register-device`
/// takes `previews` (absent keeps it) and answers `previews` +
/// `previewsAllowed`; `GET /cli/push/device` reads it back for the
/// device's own login (another Member: 404, no token or an agent
/// passport: 403); GET on `register-device` is 405; a Member can't flip
/// `pushMessagePreviews`, an Admin can, and the phone sees it as
/// `previewsAllowed:false`; `/boot-status` reports `push-previews-v1`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_previews_register_read_back_and_owner_switch() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let phone = login(d.port, "pvphone", "member");
        let other = login(d.port, "pvother", "member");
        let admin = login(d.port, "pvadmin", "admin");
        let ws = seed_ws(&format!("pvws{}", &uuid::Uuid::new_v4().to_string()[..6]));
        let agent = passport(&ws);
        let device = format!("pv-dev-{}", uuid::Uuid::new_v4());

        let r = req(d.port, "GET", &format!("/cli/push/register-device?token={phone}"), None);
        assert_eq!(r.status, 405, "GET on the POST-only route: {}", r.body);

        let register = |body: String| {
            let r = req(d.port, "POST", &format!("/cli/push/register-device?token={phone}"), Some(&body));
            assert_eq!(r.status, 200, "{}", r.body);
            r.json()
        };
        let v = register(format!(r#"{{"deviceId":"{device}","platform":"apns","token":"t1"}}"#));
        assert_eq!((v["previews"].clone(), v["previewsAllowed"].clone()), (serde_json::json!(true), serde_json::json!(true)));
        assert_eq!(v["username"], "pvphone");
        let v = register(format!(r#"{{"deviceId":"{device}","platform":"apns","token":"t2","previews":false}}"#));
        assert_eq!(v["previews"], false);
        let v = register(format!(r#"{{"deviceId":"{device}","platform":"apns","token":"t3"}}"#));
        assert_eq!(v["previews"], false, "absent keeps the stored value");

        let read = |tok: &str| req(d.port, "GET", &format!("/cli/push/device?deviceId={device}&token={tok}"), None);
        let r = read(&phone);
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!((r.json()["previews"].clone(), r.json()["previewsAllowed"].clone()), (serde_json::json!(false), serde_json::json!(true)));
        assert_eq!(read(&other).status, 404, "another Member learns nothing");
        assert_eq!(read(OWNER_TOKEN).status, 200, "the owner token can read it");
        assert_eq!(read(&agent).status, 403, "an agent passport is refused");
        let r = req(d.port, "GET", &format!("/cli/push/device?deviceId={device}"), None);
        assert_eq!(r.status, 403, "no token: {}", r.body);
        let r = req(d.port, "POST", &format!("/cli/push/device?token={phone}"), Some("{}"));
        assert_eq!(r.status, 405, "POST on the GET-only route: {}", r.body);

        let r = req(d.port, "POST", &format!("/cli/settings/update?token={other}"), Some(r#"{"pushMessagePreviews":false}"#));
        assert_eq!(r.status, 403, "a Member can't turn previews off for everyone: {}", r.body);
        let r = req(d.port, "POST", &format!("/cli/settings/update?token={admin}"), Some(r#"{"pushMessagePreviews":false}"#));
        assert_eq!(r.status, 200, "{}", r.body);
        let r = read(&phone);
        assert_eq!(r.json()["previewsAllowed"], false, "the phone greys out its toggle: {}", r.body);
        assert_eq!(r.json()["previews"], false, "its own choice is kept");

        let r = req(d.port, "GET", "/boot-status", None);
        assert_eq!(r.status, 200, "{}", r.body);
        let features = r.json()["features"].clone();
        assert!(
            features.as_array().expect("features").iter().any(|f| f == "push-previews-v1"),
            "push-previews-v1 missing: {features}"
        );
        k2_core::push_devices::remove_device(&device).expect("cleanup");
    });
}
