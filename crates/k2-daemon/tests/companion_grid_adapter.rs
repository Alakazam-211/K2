//! PR0 — companion k1 grid adapter (public `/companion/sessions/grid`).
//!
//! Drives a real companion listener (no ngrok) in front of `test_harness`
//! so the path-branch + companion-token auth + viewer-default identity
//! land in the existing `sessions_grid_ws` loop.

#![cfg(unix)]

use std::sync::Mutex as StdMutex;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use k2_core::companion::{self, TestCompanionListener};
use k2_core::terminal::Dimensions;
use k2_daemon::test_harness;
use k2_daemon::v2_session_map;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "companion-grid-owner-hook-token";
const COMPANION_TOKEN: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const OTHER_COMPANION_TOKEN: &str = "11111111-2222-3333-4444-555555555555";

type WsClient =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn http_get(port: u16, path: &str) -> (String, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
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

fn session_arc(session_id: &str) -> std::sync::Arc<k2_core::terminal::DaemonPtySession> {
    v2_session_map::lookup_by_session_id(
        &k2_core::session::SessionId::parse(session_id)
            .unwrap_or_else(|| panic!("bad session id: {session_id}")),
    )
    .unwrap_or_else(|| panic!("session {session_id} not in v2 map"))
}

fn pty_dims(session_id: &str) -> (u16, u16) {
    let s = session_arc(session_id);
    let tm = s.term();
    let t = tm.lock();
    (t.columns() as u16, t.screen_lines() as u16)
}

fn assert_dims_unchanged(session_id: &str, want: (u16, u16), what: &str) {
    let deadline = std::time::Instant::now() + Duration::from_millis(400);
    loop {
        let got = pty_dims(session_id);
        if got != want {
            panic!("{what}: PTY dims changed to {got:?} (wanted stay {want:?})");
        }
        if std::time::Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn assert_dims_settle(session_id: &str, want: (u16, u16), what: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let got = pty_dims(session_id);
        if got == want {
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!("{what}: PTY dims never settled to {want:?} (last saw {got:?})");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

async fn spawn_cat(port: u16, agent_name: &str, cols: u16, rows: u16) -> String {
    let body = serde_json::json!({
        "agent_name": agent_name,
        "cwd": std::env::temp_dir().to_string_lossy(),
        "command": "/bin/cat",
        "cols": cols,
        "rows": rows,
    })
    .to_string();
    let (status, resp) = http_post(
        port,
        &format!("/cli/sessions/v2/spawn?token={OWNER_TOKEN}"),
        &body,
    )
    .await;
    assert!(status.contains("200"), "spawn failed: {status} {resp}");
    let v: serde_json::Value =
        serde_json::from_str(&resp).unwrap_or_else(|e| panic!("spawn JSON ({e}): {resp}"));
    v["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("missing sessionId: {resp}"))
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

fn register_adapter() {
    k2_daemon::companion_host::register_grid_adapter(tokio::runtime::Handle::current());
}

struct Harness {
    daemon: test_harness::TestDaemon,
    companion: TestCompanionListener,
}

impl Harness {
    async fn start() -> Self {
        let daemon = test_harness::start(OWNER_TOKEN).await;
        register_adapter();
        let companion = TestCompanionListener::start(daemon.port, OWNER_TOKEN);
        companion.insert_session(COMPANION_TOKEN);
        companion.insert_session(OTHER_COMPANION_TOKEN);
        Self { daemon, companion }
    }

    fn companion_port(&self) -> u16 {
        self.companion.port
    }
}

fn grid_url(port: u16, session_id: &str, token: &str) -> String {
    format!("ws://127.0.0.1:{port}/companion/sessions/grid?session={session_id}&token={token}")
}

async fn connect_grid(port: u16, session_id: &str, token: &str) -> WsClient {
    let url = grid_url(port, session_id, token);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .unwrap_or_else(|e| panic!("companion grid WS connect failed: {e}"));
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

async fn reject_grid(port: u16, session_id: &str, token: &str, who: &str) -> (u16, String) {
    let url = grid_url(port, session_id, token);
    match tokio_tungstenite::connect_async(&url).await {
        Ok(_) => panic!("{who}: expected HTTP reject, got WS upgrade"),
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            let status = resp.status().as_u16();
            let body = resp
                .body()
                .as_ref()
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default();
            (status, body)
        }
        Err(e) => panic!("{who}: expected HTTP error, got {e}"),
    }
}

fn assert_no_injected_creds(haystack: &str, who: &str) {
    assert!(
        !haystack.contains(OWNER_TOKEN),
        "{who}: owner/hook token leaked: {haystack}"
    );
    assert!(
        !haystack.contains("k2st_"),
        "{who}: stream token leaked: {haystack}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn capabilities_advertises_k1_when_adapter_registered() {
    let _g = lock();
    let h = Harness::start().await;
    let (status, body) = http_get(h.companion_port(), "/companion/capabilities").await;
    assert!(
        status.contains("200"),
        "capabilities status: {status} {body}"
    );
    let v: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("capabilities JSON ({e}): {body}"));
    assert_eq!(
        v["gridProto"],
        serde_json::json!(["k1"]),
        "registered adapter must advertise k1: {v}"
    );
    assert_no_injected_creds(&body, "capabilities");
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_reject_matrix_companion_ok_hook_stream_connect_rejected() {
    let _g = lock();
    let h = Harness::start().await;
    let sid = spawn_cat(h.daemon.port, "comp-grid-auth", 80, 24).await;

    let mut ws = connect_grid(h.companion_port(), &sid, COMPANION_TOKEN).await;
    let mode = expect_event(&mut ws, "mode", "companion token").await;
    assert_eq!(mode["mode"], "viewer", "Watch-default identity: {mode}");
    assert_eq!(mode["capable"], true, "companion may Drive later: {mode}");

    let (status, body) =
        reject_grid(h.companion_port(), &sid, OWNER_TOKEN, "hook/owner token").await;
    assert!(
        status == 401 || status == 403,
        "hook token must be 401/403, got {status} {body}"
    );
    assert_no_injected_creds(&body, "hook reject body");

    let stream_tok =
        k2_daemon::stream_token::mint(&k2_core::session::SessionId::parse(&sid).expect("sid"));
    let (status, body) = reject_grid(h.companion_port(), &sid, &stream_tok, "stream token").await;
    assert!(
        status == 401 || status == 403,
        "stream token must be 401/403, got {status} {body}"
    );
    assert!(
        !body.contains(&stream_tok),
        "reject body must not echo the stream token: {body}"
    );
    k2_daemon::stream_token::revoke_for_session(
        &k2_core::session::SessionId::parse(&sid).expect("sid"),
    );

    // Connect-user tokens live in a different store and never pass
    // `validate_bearer`. A realistic-looking value is enough.
    let connect_tok = "connect-user-session-not-a-companion-token";
    let (status, body) = reject_grid(h.companion_port(), &sid, connect_tok, "Connect token").await;
    assert!(
        status == 401 || status == 403,
        "Connect token must be 401/403, got {status} {body}"
    );

    let (status, body) = reject_grid(h.companion_port(), &sid, "", "missing token").await;
    assert!(
        status == 401 || status == 403,
        "missing token must be 401/403, got {status} {body}"
    );

    close_session(h.daemon.port, "comp-grid-auth").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn watch_cannot_resize_pty_drive_set_mode_and_set_active_can() {
    let _g = lock();
    let h = Harness::start().await;
    let sid = spawn_cat(h.daemon.port, "comp-grid-watch-drive", 80, 24).await;
    assert_eq!(pty_dims(&sid), (80, 24));

    let mut ws = connect_grid(h.companion_port(), &sid, COMPANION_TOKEN).await;
    let mode = expect_event(&mut ws, "mode", "attach").await;
    assert_eq!(mode["mode"], "viewer");

    ws.send(Message::Text(
        serde_json::json!({"action":"resize","cols":40,"rows":12})
            .to_string()
            .into(),
    ))
    .await
    .expect("send watch resize");
    assert_dims_unchanged(&sid, (80, 24), "Watch resize must not SIGWINCH");

    ws.send(Message::Text(
        serde_json::json!({"action":"set_active","active":true,"cols":40,"rows":12})
            .to_string()
            .into(),
    ))
    .await
    .expect("send watch set_active");
    assert_dims_unchanged(&sid, (80, 24), "Watch set_active must not SIGWINCH");

    ws.send(Message::Text(
        serde_json::json!({"action":"set_mode","mode":"claimer"})
            .to_string()
            .into(),
    ))
    .await
    .expect("send set_mode claimer");
    let mode = expect_event(&mut ws, "mode", "drive").await;
    assert_eq!(mode["mode"], "claimer", "Drive flip: {mode}");
    assert_eq!(mode["capable"], true, "Drive capable: {mode}");

    ws.send(Message::Text(
        serde_json::json!({"action":"set_active","active":true,"cols":40,"rows":12})
            .to_string()
            .into(),
    ))
    .await
    .expect("send drive set_active");
    assert_dims_settle(&sid, (40, 12), "Drive set_mode+set_active must resize");

    close_session(h.daemon.port, "comp-grid-watch-drive").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn injected_creds_never_appear_on_the_wire() {
    let _g = lock();
    let h = Harness::start().await;
    let sid = spawn_cat(h.daemon.port, "comp-grid-creds", 80, 24).await;
    let mut ws = connect_grid(h.companion_port(), &sid, COMPANION_TOKEN).await;

    let deadline = tokio::time::Instant::now() + Duration::from_millis(400);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => assert_no_injected_creds(&t, "grid text frame"),
            Ok(Some(Ok(Message::Binary(b)))) => {
                let s = String::from_utf8_lossy(&b);
                assert_no_injected_creds(&s, "grid binary frame");
            }
            Ok(Some(Ok(_))) => {}
            _ => break,
        }
    }

    close_session(h.daemon.port, "comp-grid-creds").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_sessions_grid_is_not_exposed_on_the_tunnel() {
    let _g = lock();
    let h = Harness::start().await;
    let sid = spawn_cat(h.daemon.port, "comp-grid-no-cli", 80, 24).await;

    // `/cli/*` on the companion listener must NOT enter the daemon grid
    // loop (that would leak hook-token identity). A WS upgrade may
    // succeed as JSON-RPC, but it must never emit a grid snapshot.
    let url = format!(
        "ws://127.0.0.1:{}/cli/sessions/grid?session={sid}&token={OWNER_TOKEN}",
        h.companion_port()
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("JSON-RPC upgrade of an unknown path is still a WS");
    let saw_snapshot = tokio::time::timeout(Duration::from_millis(400), async {
        while let Some(Ok(msg)) = ws.next().await {
            if let Message::Text(t) = msg {
                if t.contains("\"event\":\"snapshot\"") {
                    return true;
                }
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(
        !saw_snapshot,
        "/cli/sessions/grid on the companion tunnel must not serve k1"
    );

    close_session(h.daemon.port, "comp-grid-no-cli").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn old_companion_ws_client_on_same_session_still_gets_scrollback() {
    let _g = lock();
    let h = Harness::start().await;
    let sid = spawn_cat(h.daemon.port, "comp-grid-scrollback", 80, 24).await;

    // New IPA: live grid WS for this terminal (marks skip for ITS token).
    let _grid = connect_grid(h.companion_port(), &sid, COMPANION_TOKEN).await;

    // Old IPA: different companion token, same terminal, `/companion/ws`.
    let url = format!(
        "ws://127.0.0.1:{}/companion/ws?token={OTHER_COMPANION_TOKEN}",
        h.companion_port()
    );
    let (mut old, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("old /companion/ws connect");
    old.send(Message::Text(
        serde_json::json!({
            "id": "1",
            "method": "terminal.subscribe",
            "params": { "terminalId": sid },
        })
        .to_string()
        .into(),
    ))
    .await
    .expect("subscribe");
    let sub_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let msg = tokio::time::timeout_at(sub_deadline, old.next())
            .await
            .expect("timed out waiting for subscribe ack")
            .expect("old WS closed before subscribe ack")
            .expect("old WS error before subscribe ack");
        if let Message::Text(t) = msg {
            if t.contains("subscribed") || t.contains("\"id\":\"1\"") {
                break;
            }
        }
    }

    // Directly exercise the skip index the poll loop uses: the NEW token
    // is marked live, the OLD token is not — scrollback must reach old.
    {
        let guard = companion::STATE.lock();
        let state = guard.as_ref().expect("companion STATE");
        assert!(
            state.client_skips_legacy_terminal(COMPANION_TOKEN, &sid),
            "grid client must be indexed for skip"
        );
        assert!(
            !state.client_skips_legacy_terminal(OTHER_COMPANION_TOKEN, &sid),
            "old IPA must not be skipped"
        );
        companion::websocket::broadcast_terminal_scrollback(
            state,
            &sid,
            &["legacy-line".to_string()],
        );
    }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut saw = false;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, old.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                if t.contains("terminal:scrollback") && t.contains("legacy-line") {
                    saw = true;
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(
        saw,
        "old /companion/ws client must still receive scrollback"
    );

    close_session(h.daemon.port, "comp-grid-scrollback").await;
}
