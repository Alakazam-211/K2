//! S7a pin-to-size (prd-presence-multiplayer-v1 §5.5) — route + wire
//! + persistence integration tests.
//!
//! Drives the REAL dispatcher (`k2_daemon::test_harness`) end-to-end:
//!
//!   - `POST /cli/terminal/pin-size` method gate (405 on GET), input
//!     validation (400 on out-of-bounds dims / unknown session), and
//!     the happy path (pin applies live + persists to
//!     `workspace_tab_sessions`, migration 0065).
//!   - A grid-WS subscriber observes `pin_changed` without polling; a
//!     LATE subscriber learns the pin from `pin_initial` at connect.
//!   - The clamp holds over a real socket: a grid-WS `resize` away
//!     from the pin never moves the PTY; clearing the pin restores
//!     that viewer's declared viewport.
//!   - A pin SURVIVES session teardown + respawn under the same
//!     `(project, pane group)` — the `v2_session_map::register`
//!     restore path (the daemon-restart stand-in).
//!
//! Fail-loudly discipline: no unwrap_or-defaults in assertions; every
//! wait has a deadline that panics listing what WAS observed.

#![cfg(unix)]

use std::sync::Mutex as StdMutex;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use k2_daemon::test_harness;
use k2_daemon::v2_session_map;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "pin-size-owner-token";

/// POST a JSON body through the real dispatcher; returns
/// `(status_line, body)`.
async fn http_post(port: u16, path_and_query: &str, body: &str) -> (String, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let req = format!(
        "POST {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).await.expect("write");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read");
    let text = String::from_utf8_lossy(&raw).into_owned();
    split_response(&text)
}

/// GET through the real dispatcher; returns `(status_line, body)`.
async fn http_get(port: u16, path_and_query: &str) -> (String, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let req = format!(
        "GET {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.expect("write");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read");
    let text = String::from_utf8_lossy(&raw).into_owned();
    split_response(&text)
}

fn split_response(text: &str) -> (String, String) {
    let status = text
        .lines()
        .next()
        .unwrap_or_else(|| panic!("empty HTTP response: {text:?}"))
        .to_string();
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_else(|| panic!("no header/body split in response: {text:?}"));
    (status, body)
}

/// Seed a `projects` row so the session's cwd resolves to a workspace
/// (pin persistence + registration restore both key off project_id).
fn setup_project(workspace_id: &str) -> std::path::PathBuf {
    let project_path = std::env::temp_dir().join(format!(
        "k2-pin-size-test-{}-{}-{}",
        workspace_id,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&project_path);
    std::fs::create_dir_all(&project_path).unwrap();

    let db = k2_core::db::init_for_tests();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects (id, path, name, agent_mode) \
         VALUES (?1, ?2, ?3, 'custom')",
        rusqlite::params![
            workspace_id,
            project_path.to_string_lossy().as_ref(),
            workspace_id,
        ],
    )
    .unwrap();
    project_path
}

/// Spawn a real `/bin/cat` PTY session via the real spawn route.
async fn spawn_cat_session(port: u16, agent_name: &str, cwd: &str) -> String {
    let body = serde_json::json!({
        "agent_name": agent_name,
        "cwd": cwd,
        "command": "/bin/cat",
        "cols": 80,
        "rows": 24,
    })
    .to_string();
    let (status, resp) = http_post(
        port,
        &format!("/cli/sessions/v2/spawn?token={OWNER_TOKEN}"),
        &body,
    )
    .await;
    assert!(status.contains("200"), "spawn failed: {status} {resp}");
    let v: serde_json::Value = serde_json::from_str(&resp)
        .unwrap_or_else(|e| panic!("spawn response not JSON ({e}): {resp}"));
    v["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("spawn response missing sessionId: {resp}"))
        .to_string()
}

type WsClient = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

/// Connect a grid-WS client and consume frames until the initial
/// snapshot arrives. Returns the socket plus every non-grid event
/// frame seen BEFORE/AFTER the snapshot up to that point (so callers
/// can assert on `pin_initial`, which follows the snapshot + label).
async fn connect_grid_client(port: u16, session_id: &str) -> WsClient {
    let url = format!(
        "ws://127.0.0.1:{port}/cli/sessions/grid?session={session_id}&token={OWNER_TOKEN}"
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("grid WS connect");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let msg = tokio::time::timeout_at(deadline, ws.next())
            .await
            .expect("timed out waiting for initial snapshot")
            .expect("WS closed before initial snapshot")
            .expect("WS error before initial snapshot");
        if let Message::Text(text) = msg {
            let v: serde_json::Value = serde_json::from_str(&text).expect("frame JSON");
            if v["event"] == "snapshot" {
                return ws;
            }
        }
    }
}

/// Drain frames until an event named `event` arrives; return its
/// payload. Panics at the deadline listing every event seen.
async fn expect_event(ws: &mut WsClient, event: &str, who: &str) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut seen: Vec<String> = Vec::new();
    loop {
        let msg = match tokio::time::timeout_at(deadline, ws.next()).await {
            Ok(Some(Ok(m))) => m,
            Ok(Some(Err(e))) => panic!("{who}: WS error waiting for {event:?}: {e}"),
            Ok(None) => panic!("{who}: WS closed waiting for {event:?}; saw: {seen:?}"),
            Err(_) => panic!(
                "{who}: never saw {event:?} within deadline; saw {} events: {seen:?}",
                seen.len()
            ),
        };
        if let Message::Text(text) = msg {
            let v: serde_json::Value = serde_json::from_str(&text).expect("frame JSON");
            let ev = v["event"].as_str().unwrap_or("").to_string();
            if ev == event {
                return v["payload"].clone();
            }
            seen.push(ev);
        }
    }
}

/// Current live Term dims for a session, read in-process.
fn live_dims(session_id: &str) -> (u16, u16) {
    use k2_core::terminal::Dimensions;
    let sid = k2_core::session::SessionId::parse(session_id).expect("valid session uuid");
    let session = v2_session_map::lookup_by_session_id(&sid)
        .unwrap_or_else(|| panic!("session {session_id} not in v2_session_map"));
    let tm = session.term();
    let t = tm.lock();
    (t.columns() as u16, t.screen_lines() as u16)
}

/// Poll until the live dims equal `want`; panic loudly otherwise.
async fn assert_dims_settle(session_id: &str, want: (u16, u16), what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut got = live_dims(session_id);
    while got != want && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(15)).await;
        got = live_dims(session_id);
    }
    assert_eq!(got, want, "{what}: dims never settled to {want:?} (last {got:?})");
}

/// Read the persisted pin columns back for `(project_id, pane_group_id)`.
fn persisted_pin(project_id: &str, pane_group_id: &str) -> Option<(u16, u16, Option<String>)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = k2_core::db::schema::WorkspaceTabSession::get(&conn, project_id, pane_group_id)
        .expect("read workspace_tab_sessions")
        .unwrap_or_else(|| {
            panic!("no workspace_tab_sessions row for ({project_id}, {pane_group_id})")
        });
    match (row.pinned_cols, row.pinned_rows) {
        (Some(c), Some(r)) => Some((c, r, row.pinned_set_by)),
        _ => None,
    }
}

async fn close_session(port: u16, agent_name: &str) {
    let (status, body) = http_post(
        port,
        &format!("/cli/sessions/v2/close?token={OWNER_TOKEN}"),
        &serde_json::json!({ "agent_name": agent_name, "force": true }).to_string(),
    )
    .await;
    assert!(status.contains("200"), "close failed: {status} {body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn pin_size_route_gates_method_and_validates_input() {
    let _g = lock();
    let _ = k2_core::db::init_for_tests();
    let daemon = test_harness::start(OWNER_TOKEN).await;

    // 405 on GET — the POST-only house rule.
    let (status, _) = http_get(
        daemon.port,
        &format!("/cli/terminal/pin-size?token={OWNER_TOKEN}"),
    )
    .await;
    assert!(status.contains("405"), "GET must 405, got: {status}");

    // 403 without a token.
    let (status, _) = http_post(daemon.port, "/cli/terminal/pin-size", "{}").await;
    assert!(status.contains("403"), "unauthenticated POST must 403, got: {status}");

    // 400 on an unknown session.
    let (status, body) = http_post(
        daemon.port,
        &format!("/cli/terminal/pin-size?token={OWNER_TOKEN}"),
        &serde_json::json!({
            "session": "11111111-2222-3333-4444-555555555555",
            "cols": 100, "rows": 30,
        })
        .to_string(),
    )
    .await;
    assert!(status.contains("400"), "unknown session must 400: {status} {body}");

    // Insane dims → 400, against a REAL session.
    let ws_id = format!("pin-gate-ws-{}", std::process::id());
    let project_path = setup_project(&ws_id);
    let session_id = spawn_cat_session(
        daemon.port,
        "tab-pin-gate",
        &project_path.to_string_lossy(),
    )
    .await;
    for (cols, rows) in [(1000u16, 30u16), (100, 999), (5, 30), (100, 2)] {
        let (status, body) = http_post(
            daemon.port,
            &format!("/cli/terminal/pin-size?token={OWNER_TOKEN}"),
            &serde_json::json!({ "session": session_id, "cols": cols, "rows": rows })
                .to_string(),
        )
        .await;
        assert!(
            status.contains("400"),
            "dims {cols}x{rows} must 400: {status} {body}"
        );
    }
    // Missing dims without clear → 400.
    let (status, body) = http_post(
        daemon.port,
        &format!("/cli/terminal/pin-size?token={OWNER_TOKEN}"),
        &serde_json::json!({ "session": session_id }).to_string(),
    )
    .await;
    assert!(status.contains("400"), "missing dims must 400: {status} {body}");
    // Nothing above may have pinned the session.
    {
        let sid = k2_core::session::SessionId::parse(&session_id).unwrap();
        let s = v2_session_map::lookup_by_session_id(&sid).expect("session live");
        assert_eq!(s.pinned(), None, "rejected requests must not pin");
    }

    close_session(daemon.port, "tab-pin-gate").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pin_applies_persists_broadcasts_and_clamps_ws_resize() {
    let _g = lock();
    let _ = k2_core::db::init_for_tests();
    let daemon = test_harness::start(OWNER_TOKEN).await;

    let ws_id = format!("pin-e2e-ws-{}", std::process::id());
    let project_path = setup_project(&ws_id);
    let pane_group = "pin-e2e-pg";
    let agent_name = format!("tab-{pane_group}");
    let session_id =
        spawn_cat_session(daemon.port, &agent_name, &project_path.to_string_lossy()).await;

    // Subscriber A attaches BEFORE the pin and declares its viewport.
    let mut client_a = connect_grid_client(daemon.port, &session_id).await;
    client_a
        .send(Message::Text(
            serde_json::json!({ "action": "resize", "cols": 80, "rows": 24 })
                .to_string()
                .into(),
        ))
        .await
        .expect("declare viewport");

    // PIN via the route.
    let (status, body) = http_post(
        daemon.port,
        &format!("/cli/terminal/pin-size?token={OWNER_TOKEN}"),
        &serde_json::json!({ "session": session_id, "cols": 100, "rows": 30 }).to_string(),
    )
    .await;
    assert!(status.contains("200"), "pin must 200: {status} {body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("pin response JSON");
    assert_eq!(v["success"], true, "pin response: {body}");
    assert_eq!(v["pinned"]["cols"], 100, "pin response: {body}");
    assert_eq!(v["pinned"]["rows"], 30, "pin response: {body}");
    assert_eq!(v["pinned"]["setBy"], "owner", "owner token attribution: {body}");
    assert_eq!(v["persisted"], true, "pin must persist: {body}");

    // Live PTY snapped to the pin.
    assert_dims_settle(&session_id, (100, 30), "pin apply").await;

    // Subscriber A observes pin_changed without polling.
    let payload = expect_event(&mut client_a, "pin_changed", "client A").await;
    assert_eq!(payload["cols"], 100, "pin_changed payload: {payload}");
    assert_eq!(payload["rows"], 30, "pin_changed payload: {payload}");
    assert_eq!(payload["cleared"], false, "pin_changed payload: {payload}");

    // Persisted row readable.
    let (p_cols, p_rows, p_by) =
        persisted_pin(&ws_id, pane_group).expect("pin row must be persisted");
    assert_eq!((p_cols, p_rows), (100, 30));
    assert_eq!(p_by.as_deref(), Some("owner"));

    // A grid-WS resize AWAY from the pin must not move the PTY —
    // send it, outwait the debounce window, assert dims held.
    client_a
        .send(Message::Text(
            serde_json::json!({ "action": "resize", "cols": 140, "rows": 45 })
                .to_string()
                .into(),
        ))
        .await
        .expect("send resize");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        live_dims(&session_id),
        (100, 30),
        "grid-WS resize must be clamped while pinned"
    );

    // Typing (the input-claim snap path) must not move it either.
    client_a
        .send(Message::Text(
            serde_json::json!({ "action": "input", "text": "hold-the-pin\r" })
                .to_string()
                .into(),
        ))
        .await
        .expect("send input");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        live_dims(&session_id),
        (100, 30),
        "typing input-claim must be clamped while pinned"
    );

    // A LATE subscriber learns the pin from pin_initial at connect.
    let mut client_b = connect_grid_client(daemon.port, &session_id).await;
    let payload = expect_event(&mut client_b, "pin_initial", "client B").await;
    assert_eq!(payload["cols"], 100, "pin_initial payload: {payload}");
    assert_eq!(payload["rows"], 30, "pin_initial payload: {payload}");
    assert_eq!(payload["set_by"], "owner", "pin_initial payload: {payload}");

    // UNPIN. The daemon restores the most recent declared viewport —
    // client A's 140x45 from the clamped resize above.
    let (status, body) = http_post(
        daemon.port,
        &format!("/cli/terminal/pin-size?token={OWNER_TOKEN}"),
        &serde_json::json!({ "session": session_id, "clear": true }).to_string(),
    )
    .await;
    assert!(status.contains("200"), "unpin must 200: {status} {body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("unpin response JSON");
    assert_eq!(v["success"], true, "unpin response: {body}");
    assert!(v["pinned"].is_null(), "unpin response: {body}");

    let payload = expect_event(&mut client_a, "pin_changed", "client A (unpin)").await;
    assert_eq!(payload["cleared"], true, "unpin payload: {payload}");
    assert!(payload.get("cols").is_none(), "cleared payload omits cols: {payload}");

    // Row cleared…
    assert_eq!(
        persisted_pin(&ws_id, pane_group),
        None,
        "unpin must clear the persisted columns"
    );
    // …and the PTY "clears back to the juggling": the last declared
    // viewport (140x45) is restored without any client resending.
    assert_dims_settle(&session_id, (140, 45), "unpin restores viewer dims").await;

    // Resizes apply normally again.
    client_a
        .send(Message::Text(
            serde_json::json!({ "action": "resize", "cols": 90, "rows": 25 })
                .to_string()
                .into(),
        ))
        .await
        .expect("send resize");
    assert_dims_settle(&session_id, (90, 25), "resize after unpin").await;

    close_session(daemon.port, &agent_name).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pin_survives_respawn_via_registration_restore() {
    let _g = lock();
    let _ = k2_core::db::init_for_tests();
    let daemon = test_harness::start(OWNER_TOKEN).await;

    let ws_id = format!("pin-respawn-ws-{}", std::process::id());
    let project_path = setup_project(&ws_id);
    let cwd = project_path.to_string_lossy().into_owned();
    let agent_name = "tab-pin-respawn-pg";

    let first = spawn_cat_session(daemon.port, agent_name, &cwd).await;
    let (status, body) = http_post(
        daemon.port,
        &format!("/cli/terminal/pin-size?token={OWNER_TOKEN}"),
        &serde_json::json!({ "session": first, "cols": 90, "rows": 25 }).to_string(),
    )
    .await;
    assert!(status.contains("200"), "pin must 200: {status} {body}");
    assert_dims_settle(&first, (90, 25), "pin on first spawn").await;

    // Tear the session down (daemon-restart stand-in: the in-memory
    // map entry is gone, only the 0065 row remains)…
    close_session(daemon.port, agent_name).await;
    let sid = k2_core::session::SessionId::parse(&first).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while v2_session_map::lookup_by_session_id(&sid).is_some() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "closed session never left the v2 map"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // …then respawn under the same (agent, cwd): registration must
    // read the persisted pin back onto the fresh DaemonPtySession.
    let second = spawn_cat_session(daemon.port, agent_name, &cwd).await;
    assert_ne!(first, second, "respawn must mint a fresh session");
    {
        let sid = k2_core::session::SessionId::parse(&second).unwrap();
        let s = v2_session_map::lookup_by_session_id(&sid).expect("respawned session live");
        assert_eq!(
            s.pinned(),
            Some((90, 25)),
            "persisted pin must be restored at registration"
        );
        assert_eq!(s.pinned_set_by().as_deref(), Some("owner"));
    }
    assert_dims_settle(&second, (90, 25), "restored pin applies to the fresh PTY").await;

    // And the restored pin still clamps: a fresh viewer's resize
    // can't move it.
    let mut client = connect_grid_client(daemon.port, &second).await;
    client
        .send(Message::Text(
            serde_json::json!({ "action": "resize", "cols": 132, "rows": 43 })
                .to_string()
                .into(),
        ))
        .await
        .expect("send resize");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        live_dims(&second),
        (90, 25),
        "restored pin must clamp a fresh viewer's resize"
    );

    close_session(daemon.port, agent_name).await;
}
