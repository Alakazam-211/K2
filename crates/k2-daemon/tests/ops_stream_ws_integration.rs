//! Phase C end-to-end integration tests for the Observability / Agent-Ops
//! fan-in WS (`/cli/ops/stream`). Mirrors `awareness_ws_integration.rs`: bind a
//! loopback listener, hand the accepted connection to the handler, then drive a
//! tokio-tungstenite client against it.
//!
//! Asserts the load-bearing Phase-C contract: ONE socket delivers BOTH a
//! `session_events` broadcast event AND an awareness-bus signal, each correctly
//! `source`-tagged, with the verbatim event nested under `event`. Also asserts
//! initial-auth rejection of an invalid token.

#![cfg(unix)]

use std::sync::Mutex as StdMutex;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

use k2_core::awareness::{self, AgentAddress, AgentSignal, SignalKind};
use k2_daemon::session_events::{self, SessionEvent};

/// Both the session_events broadcast and the awareness bus are process-wide
/// singletons — parallel tests racing subscribe/publish would cross-talk.
/// Serialize.
static OPS_TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    OPS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// One-shot listener: hand the accepted connection to
/// `serve_ops_stream_connection`. Passing the SAME value for (token,
/// owner_token) makes the owner-token short-circuit in `token_still_valid`
/// always re-validate true, so the 5s recheck never tears the socket down —
/// these tests exercise delivery, not revocation.
async fn start_ops_stream_server(token: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");
        k2_daemon::ops_stream_ws::serve_ops_stream_connection(
            &mut stream,
            token.to_string(),
            "owner".to_string(),
        )
        .await;
    });
    port
}

async fn connect_ops_ws(
    port: u16,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>> {
    let url = format!("ws://127.0.0.1:{port}/cli/ops/stream");
    let (ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("ws connect");
    ws
}

/// Read frames until one matches `pred`, ignoring the `hello` frame and any
/// other-source frames. Fails loud on timeout or stream close.
async fn next_matching<F>(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
    what: &str,
    mut pred: F,
) -> serde_json::Value
where
    F: FnMut(&serde_json::Value) -> bool,
{
    for _ in 0..10 {
        let received = timeout(Duration::from_secs(2), ws.next())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
            .expect("ws stream still open")
            .expect("message Ok");
        let text = match received {
            Message::Text(t) => t,
            other => panic!("expected Text frame, got {other:?}"),
        };
        let parsed: serde_json::Value =
            serde_json::from_str(&text).expect("frame is JSON");
        if pred(&parsed) {
            return parsed;
        }
    }
    panic!("never received a frame matching {what}");
}

#[tokio::test(flavor = "current_thread")]
async fn one_socket_delivers_both_session_and_awareness_source_tagged() {
    let _g = lock();
    let port = start_ops_stream_server("owner").await;
    let mut ws = connect_ops_ws(port).await;

    // First frame is the hello; assert it then move on.
    let hello = next_matching(&mut ws, "hello", |v| {
        v.get("kind").and_then(|k| k.as_str()) == Some("hello")
    })
    .await;
    assert!(
        hello.get("subscriber_id").and_then(|v| v.as_u64()).is_some(),
        "hello must carry a subscriber_id: {hello}",
    );

    // Give the server time to finish subscribing to BOTH buses before we emit.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 1) Emit a session_events broadcast event.
    let session_event = SessionEvent::SessionRemoved {
        workspace_path: "/x/ops-phase-c".into(),
        pane_group_id: Some("pg".into()),
        agent_name: "tab-pg".into(),
    };
    session_events::emit(session_event).expect("at least one subscriber (the WS)");

    let session_frame = next_matching(&mut ws, "session-sourced frame", |v| {
        v.get("source").and_then(|s| s.as_str()) == Some("session")
    })
    .await;
    assert_eq!(
        session_frame.pointer("/event/kind").and_then(|v| v.as_str()),
        Some("session_removed"),
        "verbatim SessionEvent must keep its `kind`: {session_frame}",
    );
    assert_eq!(
        session_frame
            .pointer("/event/workspace_path")
            .and_then(|v| v.as_str()),
        Some("/x/ops-phase-c"),
    );

    // 2) Publish an awareness-bus signal.
    let signal = AgentSignal::new(
        AgentAddress::Broadcast,
        AgentAddress::Broadcast,
        SignalKind::Msg {
            text: "ops-phase-c-awareness".into(),
        },
    );
    awareness::publish(signal.clone());

    let awareness_frame = next_matching(&mut ws, "awareness-sourced frame", |v| {
        v.get("source").and_then(|s| s.as_str()) == Some("awareness")
    })
    .await;
    assert_eq!(
        awareness_frame.pointer("/event/id").and_then(|v| v.as_str()),
        Some(signal.id.to_string().as_str()),
        "verbatim AgentSignal must be nested under `event`: {awareness_frame}",
    );

    ws.close(None).await.ok();
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_invalid_token_at_upgrade() {
    let _g = lock();
    // Server is handed a token that does NOT match the owner token, so the 5s
    // re-auth heartbeat finds it invalid and closes the socket. (The dispatcher
    // gates the upgrade itself; here we assert the in-handler re-auth fails-loud
    // for a non-owner/unknown token rather than streaming forever.)
    let port = start_ops_stream_server("not-the-owner").await;
    let mut ws = connect_ops_ws(port).await;

    // The hello still arrives (handshake succeeds before the first recheck),
    // but within ~5s the re-auth tick must close the socket because the token
    // is neither the owner token nor a live connect-user session.
    let mut closed = false;
    for _ in 0..20 {
        match timeout(Duration::from_secs(6), ws.next()).await {
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => {
                closed = true;
                break;
            }
            Ok(Some(Ok(_))) => continue, // hello / pings — keep waiting
            Ok(Some(Err(_))) => {
                closed = true;
                break;
            }
            Err(_) => panic!("re-auth heartbeat never closed an invalid-token socket"),
        }
    }
    assert!(closed, "invalid-token socket must be closed by the re-auth heartbeat");

    ws.close(None).await.ok();
}
