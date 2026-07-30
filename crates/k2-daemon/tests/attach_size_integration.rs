//! Attach-size PR2 — daemon pre-snap / reuse last known size.
//!
//! Field bug: attach snap at VT 80×24 then claim 194×62 → reflow storm
//! → k1 fat resync. Client measure-first (PR1) posts real cols/rows;
//! the daemon must honor them on spawn + reuse and pre-snap the PTY
//! before the first grid snapshot so that frame is already pane-sized.
//!
//! Pins (fail loudly):
//!   1. Fresh spawn with `{cols:194, rows:62}` → PTY + first grid snap match
//!   2. Reuse with new cols/rows → resize before first WS snapshot
//!   3. Headless omit cols/rows → still 80×24 (serde last-resort)
//!   4. Non-claimer resize still ignored (claim policy unchanged)

#![cfg(unix)]

use std::sync::Mutex as StdMutex;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use k2_core::terminal::Dimensions;
use k2_daemon::test_harness;
use k2_daemon::v2_session_map;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "attach-size-owner-token";

type WsClient = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

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

fn pty_dims(session_id: &str) -> (u16, u16) {
    let s = v2_session_map::lookup_by_session_id(
        &k2_core::session::SessionId::parse(session_id)
            .unwrap_or_else(|| panic!("bad session id: {session_id}")),
    )
    .unwrap_or_else(|| panic!("session {session_id} not in v2 map"));
    let tm = s.term();
    let t = tm.lock();
    (t.columns() as u16, t.screen_lines() as u16)
}

fn claimer_dims(session_id: &str) -> (u16, u16) {
    use std::sync::atomic::Ordering;
    let s = v2_session_map::lookup_by_session_id(
        &k2_core::session::SessionId::parse(session_id)
            .unwrap_or_else(|| panic!("bad session id: {session_id}")),
    )
    .unwrap_or_else(|| panic!("session {session_id} not in v2 map"));
    (
        s.active_cols.load(Ordering::Relaxed),
        s.active_rows.load(Ordering::Relaxed),
    )
}

async fn spawn_cat(
    port: u16,
    agent_name: &str,
    cols: Option<u16>,
    rows: Option<u16>,
) -> (String, serde_json::Value) {
    let mut body = serde_json::json!({
        "agent_name": agent_name,
        "cwd": std::env::temp_dir().to_string_lossy(),
        "command": "/bin/cat",
    });
    if let Some(c) = cols {
        body["cols"] = serde_json::json!(c);
    }
    if let Some(r) = rows {
        body["rows"] = serde_json::json!(r);
    }
    let (status, resp) = http_post(
        port,
        &format!("/cli/sessions/v2/spawn?token={OWNER_TOKEN}"),
        &body.to_string(),
    )
    .await;
    assert!(
        status.contains("200"),
        "spawn failed: {status} {resp}"
    );
    let v: serde_json::Value =
        serde_json::from_str(&resp).unwrap_or_else(|e| panic!("spawn JSON ({e}): {resp}"));
    let sid = v["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("missing sessionId: {resp}"))
        .to_string();
    (sid, v)
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

/// Connect grid-WS and return (socket, first snapshot cols, first snapshot rows).
async fn connect_grid_first_snap(port: u16, session_id: &str, token: &str) -> (WsClient, u16, u16) {
    let url = format!(
        "ws://127.0.0.1:{port}/cli/sessions/grid?session={session_id}&token={token}"
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("grid WS connect");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut seen: Vec<String> = Vec::new();
    loop {
        let msg = tokio::time::timeout_at(deadline, ws.next())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "timed out waiting for initial snapshot; saw {} frames: {seen:?}",
                    seen.len()
                )
            })
            .unwrap_or_else(|| panic!("WS closed before snapshot; saw: {seen:?}"))
            .unwrap_or_else(|e| panic!("WS error before snapshot: {e}; saw: {seen:?}"));
        if let Message::Text(text) = msg {
            let v: serde_json::Value = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("frame not JSON ({e}): {text}"));
            let ev = v["event"].as_str().unwrap_or("");
            if ev == "snapshot" {
                let cols = v["payload"]["cols"]
                    .as_u64()
                    .or_else(|| v["cols"].as_u64())
                    .unwrap_or_else(|| panic!("snapshot missing cols: {text}"))
                    as u16;
                let rows = v["payload"]["rows"]
                    .as_u64()
                    .or_else(|| v["rows"].as_u64())
                    .unwrap_or_else(|| panic!("snapshot missing rows: {text}"))
                    as u16;
                return (ws, cols, rows);
            }
            seen.push(format!("{ev}"));
        }
    }
}

/// 1. Fresh spawn with measured fit → PTY size + first snap match.
#[tokio::test(flavor = "multi_thread")]
async fn fresh_spawn_measured_fit_first_snap_matches() {
    let _g = lock();
    let daemon = test_harness::start(OWNER_TOKEN).await;
    let agent = "attach-size-fresh-194x62";
    let (sid, body) = spawn_cat(daemon.port, agent, Some(194), Some(62)).await;

    assert_eq!(body["reused"], false, "must be a fresh spawn: {body}");
    assert_eq!(body["cols"], 194, "spawn body cols: {body}");
    assert_eq!(body["rows"], 62, "spawn body rows: {body}");

    let dims = pty_dims(&sid);
    assert_eq!(
        dims,
        (194, 62),
        "fresh PTY must open at body cols/rows, not 80×24"
    );
    assert_eq!(
        claimer_dims(&sid),
        (194, 62),
        "claimer dims seeded at spawn for pre-snap"
    );

    let (_ws, snap_c, snap_r) =
        connect_grid_first_snap(daemon.port, &sid, OWNER_TOKEN).await;
    assert_eq!(
        (snap_c, snap_r),
        (194, 62),
        "first grid snapshot must already be pane-sized"
    );

    close_session(daemon.port, agent).await;
}

/// 2. Reuse with new cols/rows → resize before first WS snapshot.
#[tokio::test(flavor = "multi_thread")]
async fn reuse_new_fit_resizes_before_first_ws_snap() {
    let _g = lock();
    let daemon = test_harness::start(OWNER_TOKEN).await;
    let agent = "attach-size-reuse-fit";

    // First spawn at small size (simulates headless / prior default).
    let (sid1, body1) = spawn_cat(daemon.port, agent, Some(80), Some(24)).await;
    assert_eq!(body1["reused"], false);
    assert_eq!(pty_dims(&sid1), (80, 24));

    // Client re-spawns with measured pane fit — must reuse + resize.
    let (sid2, body2) = spawn_cat(daemon.port, agent, Some(194), Some(62)).await;
    assert_eq!(sid2, sid1, "must reuse the same session id");
    assert_eq!(body2["reused"], true, "must be reused: {body2}");
    assert_eq!(body2["cols"], 194, "reuse response cols: {body2}");
    assert_eq!(body2["rows"], 62, "reuse response rows: {body2}");
    assert_eq!(
        pty_dims(&sid2),
        (194, 62),
        "reuse must resize PTY BEFORE any grid attach"
    );
    assert_eq!(claimer_dims(&sid2), (194, 62));

    // First attach snapshot must already be at the new fit (single snap).
    let (_ws, snap_c, snap_r) =
        connect_grid_first_snap(daemon.port, &sid2, OWNER_TOKEN).await;
    assert_eq!(
        (snap_c, snap_r),
        (194, 62),
        "first WS snapshot after reuse must be at new fit (no 80×24 intermediate)"
    );

    close_session(daemon.port, agent).await;
}

/// 3. Headless / omit cols → still 80×24.
#[tokio::test(flavor = "multi_thread")]
async fn headless_omit_cols_defaults_to_80x24() {
    let _g = lock();
    let daemon = test_harness::start(OWNER_TOKEN).await;
    let agent = "attach-size-headless-default";

    let (sid, body) = spawn_cat(daemon.port, agent, None, None).await;
    assert_eq!(body["reused"], false);
    assert_eq!(
        body["cols"], 80,
        "omitted cols must serde-default to 80: {body}"
    );
    assert_eq!(
        body["rows"], 24,
        "omitted rows must serde-default to 24: {body}"
    );
    assert_eq!(
        pty_dims(&sid),
        (80, 24),
        "headless omit must open at VT 80×24 last-resort"
    );

    let (_ws, snap_c, snap_r) =
        connect_grid_first_snap(daemon.port, &sid, OWNER_TOKEN).await;
    assert_eq!((snap_c, snap_r), (80, 24));

    close_session(daemon.port, agent).await;
}

/// 4. Non-claimer resize still ignored (policy unchanged).
#[tokio::test(flavor = "multi_thread")]
async fn non_claimer_resize_still_ignored() {
    let _g = lock();
    // Seed a connect-user with viewer role so the grid connection is
    // non-claimer-capable by default (member without grant).
    let prev_home = std::env::var_os("HOME");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!(
        "k2-attach-size-viewer-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp).expect("temp HOME");
    std::env::set_var("HOME", &tmp);

    let daemon = test_harness::start(OWNER_TOKEN).await;

    // Owner spawns at 100×30; a second grid connection without claimer
    // capability must not move the PTY.
    let agent = "attach-size-non-claimer";
    let (sid, _) = spawn_cat(daemon.port, agent, Some(100), Some(30)).await;
    assert_eq!(pty_dims(&sid), (100, 30));

    // Owner claims (so active is set) then we open a second owner
    // connection that will send resize as a non-active subscriber —
    // the classic multi-client arbitration gate: only active claimer
    // resizes. Simpler path without connect-users: second owner WS
    // without set_active, first holds claim via set_active.
    let (mut owner_ws, snap_c, snap_r) =
        connect_grid_first_snap(daemon.port, &sid, OWNER_TOKEN).await;
    assert_eq!((snap_c, snap_r), (100, 30));

    owner_ws
        .send(Message::Text(
            serde_json::json!({
                "action": "set_active",
                "active": true,
                "cols": 100,
                "rows": 30,
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("set_active");
    // Give the claim a tick to land.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(pty_dims(&sid), (100, 30));

    // Second connection (also owner token → claimer-capable by default)
    // but NOT active: resize must be dropped.
    let (mut other_ws, _, _) =
        connect_grid_first_snap(daemon.port, &sid, OWNER_TOKEN).await;
    other_ws
        .send(Message::Text(
            serde_json::json!({
                "action": "resize",
                "cols": 140,
                "rows": 45,
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("non-active resize");
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        pty_dims(&sid),
        (100, 30),
        "non-active claimer resize must be ignored"
    );

    // True viewer-mode: flip second connection to viewer and try again.
    other_ws
        .send(Message::Text(
            serde_json::json!({ "action": "set_mode", "mode": "viewer" })
                .to_string()
                .into(),
        ))
        .await
        .expect("set_mode viewer");
    other_ws
        .send(Message::Text(
            serde_json::json!({
                "action": "resize",
                "cols": 160,
                "rows": 50,
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("viewer resize");
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        pty_dims(&sid),
        (100, 30),
        "viewer-mode resize must be ignored"
    );

    close_session(daemon.port, agent).await;
    let _ = other_ws;
    let _ = owner_ws;

    match prev_home {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}
