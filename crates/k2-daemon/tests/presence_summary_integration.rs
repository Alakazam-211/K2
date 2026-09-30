//! Home P1 — `GET /cli/presence/summary` through the real dispatcher.
//!
//! Contract:
//!   1. Auth: no token / garbage → 403; owner token and a connect-user
//!      session → 200; an app/skin guest pass (`k2skn_`) → 403.
//!   2. Method: POST → 405 (read-only route).
//!   3. Shape: `{ online, workspaces: [{ workspaceId, handle, name, path,
//!      count, people: [{ user, name, role }] }] }`, folded from the live
//!      `/cli/sessions/events` registry — a nested `?path=` counts on its
//!      registered workspace, a person counts once.
//!   4. `/boot-status` stays public and never carries who is online.
//!
//! ISOLATION: presence registry, connect-user stores, the shared DB, and
//! `$HOME` are process-wide. One test in this binary; `$HOME` points at a
//! fresh tempdir with the agent shim (no real `claude` can spawn).

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use k2_core::connect_users::{self, Role};
use k2_daemon::test_harness;
use rusqlite::params;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

const OWNER_TOKEN: &str = "owner-token-presence-summary-home-p1";

type WsClient = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set read timeout");
    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"),
    };
    stream.write_all(req.as_bytes()).expect("write request");
    stream.flush().expect("flush");
    let mut raw: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(r) = try_parse(&raw) {
            return r;
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof
                ) =>
            {
                break
            }
            Err(e) => panic!("read response: {e:?}"),
        }
    }
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("could not parse status from response: {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).expect("response has a body separator");
    Resp { status, body }
}

/// A complete response (Content-Length satisfied), else None.
fn try_parse(raw: &[u8]) -> Option<Resp> {
    let text = String::from_utf8_lossy(raw);
    let (headers, body) = text.split_once("\r\n\r\n")?;
    let status = headers
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())?;
    let clen = headers.lines().find_map(|l| {
        l.to_ascii_lowercase()
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    })?;
    if body.len() < clen {
        return None;
    }
    Some(Resp { status, body: body.to_string() })
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("body must be JSON ({e}): {body:?}"))
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

/// Temp `HOME` + agent shim, restored on the way out (Drop guard, so a
/// failing assertion cannot leak `HOME`).
fn with_temp_home<F: FnOnce()>(f: F) {
    struct Restore {
        prev_home: Option<std::ffi::OsString>,
        prev_shim: Option<std::ffi::OsString>,
        tmp: std::path::PathBuf,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            match self.prev_home.take() {
                Some(p) => std::env::set_var("HOME", p),
                None => std::env::remove_var("HOME"),
            }
            match self.prev_shim.take() {
                Some(p) => std::env::set_var("K2_TEST_AGENT_SHIM_DIR", p),
                None => std::env::remove_var("K2_TEST_AGENT_SHIM_DIR"),
            }
            let _ = std::fs::remove_dir_all(&self.tmp);
        }
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!("k2-presence-summary-{}-{nanos}", std::process::id()));
    let shim_dir = tmp.join("shim");
    std::fs::create_dir_all(&shim_dir).expect("create shim dir");
    let shim = shim_dir.join("claude");
    std::fs::write(&shim, "#!/bin/sh\nexec cat\n").expect("write claude shim");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod shim");
    }
    let _restore = Restore {
        prev_home: std::env::var_os("HOME"),
        prev_shim: std::env::var_os("K2_TEST_AGENT_SHIM_DIR"),
        tmp: tmp.clone(),
    };
    std::env::set_var("HOME", &tmp);
    std::env::set_var("K2_TEST_AGENT_SHIM_DIR", &shim_dir);
    f();
}

fn seed_project(handle: &str, path: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?4)",
        params![id, format!("Agent {handle}"), path, handle],
    )
    .expect("seed project");
    id
}

async fn connect_events_ws(port: u16, token: &str, path: &str) -> WsClient {
    let url = format!("ws://127.0.0.1:{port}/cli/sessions/events?path={path}&token={token}");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url).await.expect("events WS connect");
    let msg = timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timed out waiting for hello")
        .expect("stream closed before hello")
        .expect("ws message Ok");
    match msg {
        Message::Text(t) => {
            let v = json(&t);
            assert_eq!(v["kind"], "hello", "first frame must be the hello: {v}");
        }
        other => panic!("expected Text hello, got {other:?}"),
    }
    ws
}

fn summary(port: u16, token: &str) -> serde_json::Value {
    let r = http(port, "GET", &format!("/cli/presence/summary?token={token}"), None);
    assert_eq!(r.status, 200, "summary GET must be 200; body={}", r.body);
    json(&r.body)
}

fn await_registry_empty(context: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let roster = k2_daemon::presence::roster();
        if roster.is_empty() {
            return;
        }
        assert!(Instant::now() < deadline, "presence registry not empty ({context}): {roster:?}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn presence_summary_auth_shape_and_boot_status_stays_public() {
    with_temp_home(|| {
        await_registry_empty("at test start");
        let d = futures_block(test_harness::start(OWNER_TOKEN));
        let port = d.port;

        connect_users::add_user("sum_member", "password123").expect("add_user");
        connect_users::set_role("sum_member", Role::Member).expect("set_role");
        let member = connect_users::create_session("sum_member");

        let suffix = &uuid::Uuid::new_v4().to_string()[..8];
        let handle = format!("homesum{suffix}");
        let ws_path = format!("/tmp/k2-home-sum-{suffix}");
        let ws_id = seed_project(&handle, &ws_path);
        let other_handle = format!("homeidle{suffix}");
        let _idle_id = seed_project(&other_handle, &format!("/tmp/k2-home-idle-{suffix}"));

        // App/skin guest pass for the same box: must be refused.
        let r = http(
            port,
            "POST",
            &format!("/cli/skin/users?token={OWNER_TOKEN}"),
            Some(r#"{"username":"sumguest"}"#),
        );
        assert_eq!(r.status, 200, "skin user add; {}", r.body);
        let r = http(
            port,
            "POST",
            &format!("/cli/skin-tokens?token={OWNER_TOKEN}"),
            Some(&format!(
                r#"{{"name":"sum-guest","caps":["thread:read"],"rooms":["{handle}"]}}"#
            )),
        );
        assert_eq!(r.status, 200, "skin mint; {}", r.body);
        let guest = json(&r.body)["token"].as_str().expect("skin token").to_string();
        assert!(guest.starts_with("k2skn_"), "guest pass prefix: {guest}");

        // ── Auth gate ───────────────────────────────────────────────
        let r = http(port, "GET", "/cli/presence/summary", None);
        assert_eq!(r.status, 403, "no token must 403; body={}", r.body);
        let r = http(port, "GET", "/cli/presence/summary?token=garbage", None);
        assert_eq!(r.status, 403, "garbage token must 403; body={}", r.body);
        let r = http(port, "GET", &format!("/cli/presence/summary?token={guest}"), None);
        assert_eq!(r.status, 403, "app/skin guest pass must 403; body={}", r.body);
        assert!(
            !r.body.contains(&handle),
            "a refused guest must not see workspace names: {}",
            r.body
        );

        // ── Method gate ─────────────────────────────────────────────
        let r = http(
            port,
            "POST",
            &format!("/cli/presence/summary?token={OWNER_TOKEN}"),
            Some("{}"),
        );
        assert_eq!(r.status, 405, "POST must 405; body={}", r.body);

        // ── Empty: nobody connected ─────────────────────────────────
        let v = summary(port, OWNER_TOKEN);
        assert_eq!(v["online"], 0, "{v}");
        assert_eq!(v["workspaces"], serde_json::json!([]), "{v}");

        futures_block(async {
            // Owner on the workspace itself, member on a nested path
            // under it, plus each one's app-level socket.
            let ws_owner_app = connect_events_ws(port, OWNER_TOKEN, "").await;
            let ws_owner = connect_events_ws(port, OWNER_TOKEN, &ws_path).await;
            let ws_member = connect_events_ws(port, &member, &format!("{ws_path}/sub")).await;

            let v = summary(port, OWNER_TOKEN);
            assert_eq!(v["online"], 2, "two distinct people: {v}");
            let list = v["workspaces"].as_array().expect("workspaces array");
            assert_eq!(list.len(), 1, "only the occupied workspace is listed: {v}");
            let w = &list[0];
            assert_eq!(w["workspaceId"], ws_id.as_str(), "{w}");
            assert_eq!(w["handle"], handle.as_str(), "{w}");
            assert_eq!(w["name"], format!("Agent {handle}"), "{w}");
            assert_eq!(w["path"], ws_path.as_str(), "{w}");
            assert_eq!(w["count"], 2, "{w}");
            assert_eq!(
                w["people"],
                serde_json::json!([
                    { "user": "owner", "name": "owner", "role": "owner" },
                    { "user": "sum_member", "name": "sum_member", "role": "member" },
                ]),
                "{w}"
            );
            assert!(
                !v.to_string().contains(&other_handle),
                "an unoccupied workspace must not be listed: {v}"
            );

            // Same answer through the connect-user session.
            assert_eq!(summary(port, &member), v);

            // /boot-status is public and carries no presence.
            let b = http(port, "GET", "/boot-status", None);
            assert_eq!(b.status, 200, "boot-status; {}", b.body);
            assert!(
                !b.body.contains("sum_member") && !b.body.contains("people"),
                "boot-status must not carry who is online: {}",
                b.body
            );

            drop(ws_member);
            drop(ws_owner);
            drop(ws_owner_app);
        });

        await_registry_empty("after closing sockets");
        let v = summary(port, OWNER_TOKEN);
        assert_eq!(v["online"], 0, "{v}");
        assert_eq!(v["workspaces"], serde_json::json!([]), "{v}");
    });
}
