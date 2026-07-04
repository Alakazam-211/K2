//! S5 viewer/claimer enforcement (prd-presence-multiplayer-v1 §4 + §5.3)
//! — grid-WS gate integration tests.
//!
//! Drives the REAL dispatcher (`k2_daemon::test_harness`) end-to-end,
//! cribbing the S3 kick harness (temp `$HOME`, seeded connect-user
//! sessions, events-WS presence registration) and the S7a pin harness
//! (real `/bin/cat` PTY spawn, grid-WS client, in-process Term reads):
//!
//!   1. a VIEWER-role user's grid connection: input dropped (the PTY
//!      echo never reaches the grid), resize ignored (dims unchanged),
//!      `input_denied` received exactly ONCE per connection;
//!   2. after `POST /cli/presence/grant` the SAME connection's input
//!      flows (capability is computed per frame, not at accept);
//!   3. revoking the grant blocks the SAME connection again;
//!   4. a MEMBER in viewer MODE (the non-owner default) is blocked;
//!      flipping `set_mode` to claimer lets input/resize flow; flipping
//!      back re-blocks;
//!   5. an OWNER connection defaults to claimer and is unaffected.
//!
//! Fail-loudly discipline: no unwrap_or-defaults in assertions; every
//! wait has a deadline that panics listing what WAS observed.

#![cfg(unix)]

use std::sync::Mutex as StdMutex;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use k2_core::connect_users::{self, Role};
use k2_daemon::test_harness;
use k2_daemon::v2_session_map;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "viewer-claimer-owner-token";

type WsClient = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

// ── HTTP + harness helpers (pin_size harness style) ──────────────────

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

/// Redirect `$HOME` to a fresh tempdir and run `f` (kick-suite pattern —
/// the connect-user store lives under `$HOME`). Caller holds `TEST_LOCK`.
fn with_temp_home<F: FnOnce()>(f: F) {
    let prev = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("k2-viewer-s5-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("create temp HOME");
    std::env::set_var("HOME", &tmp);

    f();

    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Seed a connect-user with a role and return a live session token.
fn seed_user_session(username: &str, password: &str, role: Role) -> String {
    connect_users::add_user(username, password).expect("add_user");
    connect_users::set_role(username, role).expect("set_role");
    connect_users::create_session(username)
}

fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}

/// Seed a `projects` row so the session's cwd resolves to a workspace.
fn setup_project(workspace_id: &str) -> std::path::PathBuf {
    let project_path = std::env::temp_dir().join(format!(
        "k2-viewer-s5-test-{}-{}-{}",
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

async fn close_session(port: u16, agent_name: &str) {
    let (status, body) = http_post(
        port,
        &format!("/cli/sessions/v2/close?token={OWNER_TOKEN}"),
        &serde_json::json!({ "agent_name": agent_name, "force": true }).to_string(),
    )
    .await;
    assert!(status.contains("200"), "close failed: {status} {body}");
}

/// Open a `/cli/sessions/events` WS with `token` and consume the `hello`
/// frame — after which the connection is REGISTERED in the presence
/// registry (the grant route requires a live presence connection).
async fn connect_events_ws(port: u16, token: &str) -> WsClient {
    let url = format!("ws://127.0.0.1:{port}/cli/sessions/events?path=&token={token}");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("events WS connect");
    let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timed out waiting for hello")
        .expect("stream closed before hello")
        .expect("ws message Ok");
    let hello: serde_json::Value = match msg {
        Message::Text(t) => serde_json::from_str(&t).expect("frame is JSON"),
        other => panic!("expected Text hello frame, got {other:?}"),
    };
    assert_eq!(hello["kind"], "hello", "first frame must be the hello: {hello}");
    ws
}

/// Connect a grid-WS client with `token` and consume frames until the
/// initial snapshot arrives (the mode ACK follows the snapshot in the
/// initial-state slot, so callers `expect_event(.., "mode", ..)` next).
async fn connect_grid_client(port: u16, session_id: &str, token: &str) -> WsClient {
    let url = format!(
        "ws://127.0.0.1:{port}/cli/sessions/grid?session={session_id}&token={token}"
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

/// Drain everything that arrives within `window` and count frames whose
/// event name is `event`. Used to prove `input_denied` is one-time.
async fn count_event_within(ws: &mut WsClient, event: &str, window: Duration) -> usize {
    let deadline = tokio::time::Instant::now() + window;
    let mut count = 0usize;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return count;
        }
        match tokio::time::timeout(remaining, ws.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                let v: serde_json::Value =
                    serde_json::from_str(&text).expect("frame JSON");
                if v["event"] == event {
                    count += 1;
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => panic!("WS error while draining: {e}"),
            Ok(None) => panic!("WS closed while draining for {event:?}"),
            Err(_) => return count,
        }
    }
}

/// All text currently on the session's Term (scrollback + live grid),
/// read in-process. `/bin/cat`'s PTY runs in canonical mode with ECHO,
/// so any input the daemon actually writes shows up here.
fn grid_text(session_id: &str) -> String {
    let sid = k2_core::session::SessionId::parse(session_id).expect("valid session uuid");
    let session = v2_session_map::lookup_by_session_id(&sid)
        .unwrap_or_else(|| panic!("session {session_id} not in v2_session_map"));
    let tm = session.term();
    let t = tm.lock();
    let snap = k2_core::terminal::snapshot_term("s5-probe", &t, 0);
    let mut out = String::new();
    for row in snap.scrollback.iter().chain(snap.grid.iter()) {
        for run in row {
            out.push_str(&run.text);
        }
        out.push('\n');
    }
    out
}

/// Poll until `needle` shows on the Term; panic loudly at the deadline.
async fn assert_text_appears(session_id: &str, needle: &str, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if grid_text(session_id).contains(needle) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what}: {needle:?} never appeared on the Term; grid was:\n{}",
            grid_text(session_id)
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Outwait the write/echo path, then assert `needle` is ABSENT.
async fn assert_text_never_appears(session_id: &str, needle: &str, what: &str) {
    tokio::time::sleep(Duration::from_millis(500)).await;
    let text = grid_text(session_id);
    assert!(
        !text.contains(needle),
        "{what}: {needle:?} reached the PTY (must have been dropped); grid:\n{text}"
    );
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

async fn send_json(ws: &mut WsClient, value: serde_json::Value, what: &str) {
    ws.send(Message::Text(value.to_string().into()))
        .await
        .unwrap_or_else(|e| panic!("{what}: send failed: {e}"));
}

async fn set_grant(port: u16, username: &str, granted: bool) {
    let (status, body) = http_post(
        port,
        &format!("/cli/presence/grant?token={OWNER_TOKEN}"),
        &serde_json::json!({ "username": username, "granted": granted }).to_string(),
    )
    .await;
    assert!(status.contains("200"), "grant({granted}) failed: {status} {body}");
}

// ─────────────────────────────────────────────────────────────────────
// 1+2+3 — viewer role: gated; grant unlocks the LIVE connection;
//         revoke re-blocks it (dynamic capability)
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn viewer_role_gated_then_grant_flows_then_revoke_blocks() {
    let _g = lock();
    with_temp_home(|| {
        let viewer_tok = seed_user_session("s5_viewer", "password123", Role::Viewer);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        futures_block(async {
            let ws_id = format!("s5-viewer-ws-{}", std::process::id());
            let project_path = setup_project(&ws_id);
            let agent = "tab-s5-viewer";
            let session_id =
                spawn_cat_session(d.port, agent, &project_path.to_string_lossy()).await;

            // Presence registration — the grant route requires the
            // target to hold a live presence connection.
            let _events_ws = connect_events_ws(d.port, &viewer_tok).await;

            let mut grid = connect_grid_client(d.port, &session_id, &viewer_tok).await;

            // Connect-time mode ACK: non-owner default is viewer, and a
            // viewer-role user without a grant is not capable.
            let mode = expect_event(&mut grid, "mode", "viewer connect").await;
            assert_eq!(mode["mode"], "viewer", "connect ACK: {mode}");
            assert_eq!(mode["capable"], false, "connect ACK: {mode}");

            // (1) Input dropped + one-time input_denied.
            send_json(
                &mut grid,
                serde_json::json!({ "action": "input", "text": "S5_DENY_ONE\r" }),
                "viewer input 1",
            )
            .await;
            let denied = expect_event(&mut grid, "input_denied", "viewer input").await;
            assert_eq!(denied["reason"], "viewer", "input_denied payload: {denied}");
            // A second dropped input must NOT produce a second frame.
            send_json(
                &mut grid,
                serde_json::json!({ "action": "input", "text": "S5_DENY_TWO\r" }),
                "viewer input 2",
            )
            .await;
            let repeats =
                count_event_within(&mut grid, "input_denied", Duration::from_millis(600))
                    .await;
            assert_eq!(repeats, 0, "input_denied must be one-time per connection");
            assert_text_never_appears(&session_id, "S5_DENY_ONE", "viewer input 1").await;
            assert_text_never_appears(&session_id, "S5_DENY_TWO", "viewer input 2").await;

            // (1b) Resize ignored — dims stay at the spawn size.
            send_json(
                &mut grid,
                serde_json::json!({ "action": "resize", "cols": 100, "rows": 30 }),
                "viewer resize",
            )
            .await;
            tokio::time::sleep(Duration::from_millis(400)).await;
            assert_eq!(
                live_dims(&session_id),
                (80, 24),
                "viewer resize must be ignored"
            );

            // (2) GRANT → the SAME connection becomes capable. Flip the
            // window mode to claimer and both input + resize flow.
            set_grant(d.port, "s5_viewer", true).await;
            send_json(
                &mut grid,
                serde_json::json!({ "action": "set_mode", "mode": "claimer" }),
                "set_mode claimer",
            )
            .await;
            let mode = expect_event(&mut grid, "mode", "post-grant set_mode").await;
            assert_eq!(mode["mode"], "claimer", "post-grant ACK: {mode}");
            assert_eq!(mode["capable"], true, "post-grant ACK: {mode}");
            send_json(
                &mut grid,
                serde_json::json!({ "action": "input", "text": "S5_GRANTED\r" }),
                "granted input",
            )
            .await;
            assert_text_appears(&session_id, "S5_GRANTED", "granted input").await;
            send_json(
                &mut grid,
                serde_json::json!({ "action": "resize", "cols": 100, "rows": 30 }),
                "granted resize",
            )
            .await;
            assert_dims_settle(&session_id, (100, 30), "granted resize").await;

            // (3) REVOKE → the SAME connection (mode still claimer) is
            // blocked again: capability is recomputed per frame.
            set_grant(d.port, "s5_viewer", false).await;
            send_json(
                &mut grid,
                serde_json::json!({ "action": "input", "text": "S5_REVOKED\r" }),
                "post-revoke input",
            )
            .await;
            assert_text_never_appears(&session_id, "S5_REVOKED", "post-revoke input")
                .await;
            send_json(
                &mut grid,
                serde_json::json!({ "action": "resize", "cols": 90, "rows": 25 }),
                "post-revoke resize",
            )
            .await;
            tokio::time::sleep(Duration::from_millis(400)).await;
            assert_eq!(
                live_dims(&session_id),
                (100, 30),
                "post-revoke resize must be ignored"
            );
            // And the ACK now reports the truth: mode stored claimer,
            // capable false.
            send_json(
                &mut grid,
                serde_json::json!({ "action": "set_mode", "mode": "claimer" }),
                "post-revoke set_mode",
            )
            .await;
            let mode = expect_event(&mut grid, "mode", "post-revoke set_mode").await;
            assert_eq!(mode["mode"], "claimer", "post-revoke ACK: {mode}");
            assert_eq!(mode["capable"], false, "post-revoke ACK: {mode}");

            close_session(d.port, agent).await;
        });
    });
}

// ─────────────────────────────────────────────────────────────────────
// 4 — member: viewer MODE blocks a capable user; set_mode flips it
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_blocked_in_viewer_mode_flows_after_claimer_flip() {
    let _g = lock();
    with_temp_home(|| {
        let member_tok = seed_user_session("s5_member", "password123", Role::Member);
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        futures_block(async {
            let ws_id = format!("s5-member-ws-{}", std::process::id());
            let project_path = setup_project(&ws_id);
            let agent = "tab-s5-member";
            let session_id =
                spawn_cat_session(d.port, agent, &project_path.to_string_lossy()).await;

            let mut grid = connect_grid_client(d.port, &session_id, &member_tok).await;

            // Non-owner default mode is viewer — but a member IS capable.
            let mode = expect_event(&mut grid, "mode", "member connect").await;
            assert_eq!(mode["mode"], "viewer", "connect ACK: {mode}");
            assert_eq!(mode["capable"], true, "connect ACK: {mode}");

            // Viewer MODE blocks even a capable role (mode AND capability).
            send_json(
                &mut grid,
                serde_json::json!({ "action": "input", "text": "S5_MEMBER_HIDDEN\r" }),
                "member viewer-mode input",
            )
            .await;
            let denied =
                expect_event(&mut grid, "input_denied", "member viewer-mode").await;
            assert_eq!(denied["reason"], "viewer", "payload: {denied}");
            assert_text_never_appears(
                &session_id,
                "S5_MEMBER_HIDDEN",
                "member viewer-mode input",
            )
            .await;

            // Flip to claimer → flows.
            send_json(
                &mut grid,
                serde_json::json!({ "action": "set_mode", "mode": "claimer" }),
                "member set_mode claimer",
            )
            .await;
            let mode = expect_event(&mut grid, "mode", "member set_mode").await;
            assert_eq!(mode["mode"], "claimer", "ACK: {mode}");
            assert_eq!(mode["capable"], true, "ACK: {mode}");
            send_json(
                &mut grid,
                serde_json::json!({ "action": "input", "text": "S5_MEMBER_SHOWN\r" }),
                "member claimer input",
            )
            .await;
            assert_text_appears(&session_id, "S5_MEMBER_SHOWN", "member claimer input")
                .await;
            send_json(
                &mut grid,
                serde_json::json!({ "action": "resize", "cols": 120, "rows": 36 }),
                "member claimer resize",
            )
            .await;
            assert_dims_settle(&session_id, (120, 36), "member claimer resize").await;

            // Flip back to viewer → blocked again.
            send_json(
                &mut grid,
                serde_json::json!({ "action": "set_mode", "mode": "viewer" }),
                "member set_mode viewer",
            )
            .await;
            let mode = expect_event(&mut grid, "mode", "member set_mode viewer").await;
            assert_eq!(mode["mode"], "viewer", "ACK: {mode}");
            assert_eq!(mode["capable"], true, "ACK: {mode}");
            send_json(
                &mut grid,
                serde_json::json!({ "action": "input", "text": "S5_MEMBER_AGAIN\r" }),
                "member re-blocked input",
            )
            .await;
            assert_text_never_appears(
                &session_id,
                "S5_MEMBER_AGAIN",
                "member re-blocked input",
            )
            .await;

            close_session(d.port, agent).await;
        });
    });
}

// ─────────────────────────────────────────────────────────────────────
// 5 — owner: default claimer, completely unaffected by S5
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_defaults_to_claimer_and_is_unaffected() {
    let _g = lock();
    with_temp_home(|| {
        let d = futures_block(test_harness::start(OWNER_TOKEN));

        futures_block(async {
            let ws_id = format!("s5-owner-ws-{}", std::process::id());
            let project_path = setup_project(&ws_id);
            let agent = "tab-s5-owner";
            let session_id =
                spawn_cat_session(d.port, agent, &project_path.to_string_lossy()).await;

            let mut grid = connect_grid_client(d.port, &session_id, OWNER_TOKEN).await;

            // Owner default: claimer + capable, straight from connect.
            let mode = expect_event(&mut grid, "mode", "owner connect").await;
            assert_eq!(mode["mode"], "claimer", "connect ACK: {mode}");
            assert_eq!(mode["capable"], true, "connect ACK: {mode}");

            // Input + resize flow with no set_mode ever sent (an older
            // client's behavior is byte-identical to pre-S5).
            send_json(
                &mut grid,
                serde_json::json!({ "action": "input", "text": "S5_OWNER_TYPES\r" }),
                "owner input",
            )
            .await;
            assert_text_appears(&session_id, "S5_OWNER_TYPES", "owner input").await;
            send_json(
                &mut grid,
                serde_json::json!({ "action": "resize", "cols": 132, "rows": 43 }),
                "owner resize",
            )
            .await;
            assert_dims_settle(&session_id, (132, 43), "owner resize").await;

            close_session(d.port, agent).await;
        });
    });
}
