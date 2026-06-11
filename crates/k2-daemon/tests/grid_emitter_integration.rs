//! 0.39.46 — shared grid emitter regression tests ("starved viewer").
//!
//! The bug: every grid-WS connection ran its own `build_emit()` against
//! the session's Term, but alacritty's damage tracker is ONE shared
//! accumulator — with two subscribers, each PTY wakeup was a race and
//! the losing connection's mirror silently missed rows (the remote
//! viewer's "missing line" on input wrap, healed only by tab-switch).
//!
//! These tests spawn a REAL PTY session through the real spawn route,
//! attach TWO real grid-WS clients through the real dispatcher, write
//! input, and assert BOTH clients observe the new content. Pre-fix this
//! failed (racily, i.e. constantly in practice) for one of the two.
//!
//! Fail-loudly discipline: assertions have no fallbacks; a client that
//! never converges panics with the frames it did receive.

#![cfg(unix)]

use std::sync::Mutex as StdMutex;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use k2_daemon::test_harness;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const OWNER_TOKEN: &str = "grid-emitter-owner-token";

/// POST a JSON body through the real dispatcher; return the body text.
async fn http_post(port: u16, path_and_query: &str, body: &str) -> String {
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
    let text = String::from_utf8_lossy(&raw);
    text.split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default()
}

/// Spawn a real `/bin/cat` PTY session; returns its session id.
async fn spawn_cat_session(port: u16, agent_name: &str) -> String {
    let body = serde_json::json!({
        "agent_name": agent_name,
        "cwd": std::env::temp_dir().to_string_lossy(),
        "command": "/bin/cat",
        "cols": 80,
        "rows": 24,
    })
    .to_string();
    let resp = http_post(
        port,
        &format!("/cli/sessions/v2/spawn?token={OWNER_TOKEN}"),
        &body,
    )
    .await;
    let v: serde_json::Value =
        serde_json::from_str(&resp).unwrap_or_else(|e| panic!("spawn response not JSON ({e}): {resp}"));
    v["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("spawn response missing sessionId: {resp}"))
        .to_string()
}

type WsClient = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

/// Connect a grid-WS client and consume frames until the initial
/// snapshot arrives (the protocol guarantees it's the first grid
/// frame; label frames may interleave).
async fn connect_grid_client(port: u16, session_id: &str) -> WsClient {
    let url = format!(
        "ws://127.0.0.1:{port}/cli/sessions/grid?session={session_id}&token={OWNER_TOKEN}"
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("grid WS connect");
    // Wait for the initial snapshot.
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

/// Drain frames from `ws` until one contains `needle` (in any grid
/// frame's serialized text — snapshot or delta), or panic at the
/// deadline listing what WAS received.
async fn expect_text(ws: &mut WsClient, needle: &str, who: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut seen: Vec<String> = Vec::new();
    loop {
        let msg = match tokio::time::timeout_at(deadline, ws.next()).await {
            Ok(Some(Ok(m))) => m,
            Ok(Some(Err(e))) => panic!("{who}: WS error while waiting for {needle:?}: {e}"),
            Ok(None) => panic!("{who}: WS closed while waiting for {needle:?}; saw: {seen:?}"),
            Err(_) => panic!(
                "{who}: STARVED — never saw {needle:?} within deadline; received {} frames: {:?}",
                seen.len(),
                seen.iter().map(|s| &s[..s.len().min(120)]).collect::<Vec<_>>()
            ),
        };
        if let Message::Text(text) = msg {
            let v: serde_json::Value = serde_json::from_str(&text).expect("frame JSON");
            let ev = v["event"].as_str().unwrap_or("");
            if (ev == "snapshot" || ev == "delta") && text.contains(needle) {
                return;
            }
            seen.push(format!("{ev}:{}", &text[..text.len().min(80)]));
        }
    }
}

/// THE regression test: two simultaneous viewers, input through one,
/// BOTH must render it. Pre-0.39.46 the damage race starved one of
/// them on most rounds.
#[tokio::test(flavor = "multi_thread")]
async fn two_subscribers_both_observe_typed_input() {
    let _g = lock();
    let daemon = test_harness::start(OWNER_TOKEN).await;
    let session_id = spawn_cat_session(daemon.port, "grid-emitter-test-two-subs").await;

    let mut client_a = connect_grid_client(daemon.port, &session_id).await;
    let mut client_b = connect_grid_client(daemon.port, &session_id).await;

    // Three rounds: repeated wakeups are what exercised the race.
    for round in 0..3 {
        let marker = format!("starve-check-{round}-zq7");
        client_a
            .send(Message::Text(
                serde_json::json!({ "action": "input", "text": format!("{marker}\r") })
                    .to_string()
                    .into(),
            ))
            .await
            .expect("send input");

        // PTY echo paints the marker into the grid; both viewers must
        // converge on it.
        expect_text(&mut client_a, &marker, "client A (typist)").await;
        expect_text(&mut client_b, &marker, "client B (watcher)").await;
    }

    // Teardown: force-close the session so the cat child is reaped.
    let _ = http_post(
        daemon.port,
        &format!("/cli/sessions/v2/close?token={OWNER_TOKEN}"),
        &serde_json::json!({ "session_id": session_id, "force": true }).to_string(),
    )
    .await;
}

/// A LATE-attaching second viewer must get a coherent picture: its
/// initial snapshot already contains earlier output, and it must not
/// receive duplicate scrollback (the version-floor contract), while
/// still converging on output produced AFTER it attached.
#[tokio::test(flavor = "multi_thread")]
async fn late_attacher_converges_without_starving_the_first() {
    let _g = lock();
    let daemon = test_harness::start(OWNER_TOKEN).await;
    let session_id = spawn_cat_session(daemon.port, "grid-emitter-test-late-attach").await;

    let mut client_a = connect_grid_client(daemon.port, &session_id).await;

    // A types BEFORE B attaches.
    let early = "early-marker-9fk3";
    client_a
        .send(Message::Text(
            serde_json::json!({ "action": "input", "text": format!("{early}\r") })
                .to_string()
                .into(),
        ))
        .await
        .expect("send input");
    expect_text(&mut client_a, early, "client A pre-attach").await;

    // B attaches late — its INITIAL snapshot must already show the
    // early marker (connect_grid_client returns after the snapshot;
    // re-verify by checking the snapshot carried it).
    let url = format!(
        "ws://127.0.0.1:{port}/cli/sessions/grid?session={session_id}&token={OWNER_TOKEN}",
        port = daemon.port
    );
    let (mut client_b, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("late client connect");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let msg = tokio::time::timeout_at(deadline, client_b.next())
            .await
            .expect("timed out waiting for late snapshot")
            .expect("WS closed")
            .expect("WS error");
        if let Message::Text(text) = msg {
            let v: serde_json::Value = serde_json::from_str(&text).expect("frame JSON");
            if v["event"] == "snapshot" {
                assert!(
                    text.contains(early),
                    "late attacher's initial snapshot must contain pre-attach output"
                );
                break;
            }
        }
    }

    // New input after B attached: BOTH converge. Critically, A (the
    // pre-existing subscriber) must not have been starved by B's
    // attach — pre-0.39.46 the attach path reset the Term's damage.
    let late = "late-marker-2vx8";
    client_a
        .send(Message::Text(
            serde_json::json!({ "action": "input", "text": format!("{late}\r") })
                .to_string()
                .into(),
        ))
        .await
        .expect("send input");
    expect_text(&mut client_a, late, "client A post-attach").await;
    expect_text(&mut client_b, late, "client B late-attacher").await;

    let _ = http_post(
        daemon.port,
        &format!("/cli/sessions/v2/close?token={OWNER_TOKEN}"),
        &serde_json::json!({ "session_id": session_id, "force": true }).to_string(),
    )
    .await;
}
