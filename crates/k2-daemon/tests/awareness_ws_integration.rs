//! E7 end-to-end integration tests for the awareness WS subscribe
//! path. Mirrors the D6 sessions_ws_integration harness — bind a
//! loopback listener, hand the accepted connection to the handler,
//! then drive a tokio-tungstenite client against it.

#![cfg(unix)]

use std::sync::Mutex as StdMutex;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

use k2_core::awareness::{
    self, AgentAddress, AgentSignal, SignalKind,
};

/// The bus is a process-wide singleton — parallel tests racing
/// subscribe/publish would see each other's signals and cause
/// id-mismatch false failures. Serialize.
static BUS_TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    BUS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Spin up a one-shot listener, hand the accepted connection to
/// `serve_awareness_subscribe_connection`. Returns the bound port.
async fn start_awareness_subscribe_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");
        // 0.39.7: WS handlers now borrow the stream (so the daemon's
        // keep-alive dispatcher can own it across HTTP iterations).
        // Test owns its own stream and passes a mutable borrow.
        k2_daemon::awareness_ws::serve_awareness_subscribe_connection(&mut stream).await;
    });
    port
}

async fn connect_awareness_ws(
    port: u16,
) -> tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<TcpStream>,
> {
    let url = format!("ws://127.0.0.1:{port}/cli/awareness/subscribe");
    let (ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("ws connect");
    ws
}

#[tokio::test(flavor = "current_thread")]
async fn subscriber_receives_published_signal() {
    let _g = lock();
    let port = start_awareness_subscribe_server().await;
    let mut ws = connect_awareness_ws(port).await;

    // Give the server time to complete the upgrade + subscribe
    // before we publish.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let signal = AgentSignal::new(
        AgentAddress::Broadcast,
        AgentAddress::Agent {
            workspace: awareness::WorkspaceId("k2so".into()),
            name: "bar".into(),
        },
        SignalKind::Msg {
            text: "e7-integration-test".into(),
        },
    );
    awareness::publish(signal.clone());

    let received = timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("no timeout")
        .expect("ws stream open")
        .expect("message Ok");

    let text = match received {
        Message::Text(t) => t,
        other => panic!("expected Text, got {other:?}"),
    };
    let parsed: serde_json::Value =
        serde_json::from_str(&text).expect("JSON parse");
    assert_eq!(
        parsed.get("event").and_then(|v| v.as_str()),
        Some("awareness:signal")
    );
    assert_eq!(
        parsed.pointer("/payload/id").and_then(|v| v.as_str()),
        Some(signal.id.to_string().as_str())
    );

    ws.close(None).await.ok();
}

#[tokio::test(flavor = "current_thread")]
async fn multiple_subscribers_each_receive_signals() {
    let _g = lock();
    let port_a = start_awareness_subscribe_server().await;
    let port_b = start_awareness_subscribe_server().await;
    let mut ws_a = connect_awareness_ws(port_a).await;
    let mut ws_b = connect_awareness_ws(port_b).await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let signal = AgentSignal::new(
        AgentAddress::Broadcast,
        AgentAddress::Broadcast,
        SignalKind::Status {
            text: "e7-multi-sub".into(),
        },
    );
    awareness::publish(signal.clone());

    // Both receivers get it.
    for ws in [&mut ws_a, &mut ws_b] {
        let received = timeout(Duration::from_secs(2), ws.next())
            .await
            .expect("no timeout")
            .expect("ws open")
            .expect("msg Ok");
        match received {
            Message::Text(t) => {
                assert!(t.contains("e7-multi-sub") || t.contains(&signal.id.to_string()));
            }
            other => panic!("unexpected frame {other:?}"),
        }
    }

    ws_a.close(None).await.ok();
    ws_b.close(None).await.ok();
}
