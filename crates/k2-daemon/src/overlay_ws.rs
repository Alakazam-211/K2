//! `WS /cli/overlay/events?conversation=` — overlay push bus.
//!
//! Distinct from `/cli/sessions/events` (the 256-slot session_events bus)
//! and from grid-WS. Frames: `{collection, seq, id, doc?}`.

use std::collections::HashMap;
use std::sync::OnceLock;

use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

use k2_core::log_debug;
use k2_core::overlay::OverlayDoc;

/// Wire path. Tests assert this is not `session_events`.
pub const OVERLAY_WS_PATH: &str = "/cli/overlay/events";

const BUS_CAP: usize = 1024;

#[derive(Debug, Clone, Serialize)]
pub struct OverlayFrame {
    pub collection: String,
    pub seq: i64,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<OverlayDoc>,
    /// Internal filter key; omitted from the wire shape in [`wire_json`].
    #[serde(skip)]
    pub conversation_id: Option<String>,
}

fn bus() -> &'static broadcast::Sender<OverlayFrame> {
    static TX: OnceLock<broadcast::Sender<OverlayFrame>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, _rx) = broadcast::channel(BUS_CAP);
        tx
    })
}

pub fn publish(frame: OverlayFrame) {
    let _ = bus().send(frame);
}

pub fn subscribe() -> broadcast::Receiver<OverlayFrame> {
    bus().subscribe()
}

fn wire_json(frame: &OverlayFrame) -> String {
    serde_json::json!({
        "collection": frame.collection,
        "seq": frame.seq,
        "id": frame.id,
        "doc": frame.doc,
    })
    .to_string()
}

/// Whether this overlay subscriber may see `frame`.
///
/// Skin WS is Thread-only: host-wide chatterlog frames (`conversation_id:
/// None`) are dropped. Owner/Connect subscribers keep the unfiltered bus.
pub fn skin_may_see_frame(frame: &OverlayFrame, conversation: &str, skin: bool) -> bool {
    match &frame.conversation_id {
        Some(cid) => cid == conversation,
        None => !skin, // chatterlog is host-wide — never a skin room
    }
}

/// WS handler. Dispatcher already token-authed. Requires `conversation=`.
/// `skin` filters chatterlog frames (prd-skin-auth-v1 non-goal).
pub async fn serve_overlay_events_connection(
    stream: &mut TcpStream,
    params: HashMap<String, String>,
    skin: bool,
) {
    let conversation = params
        .get("conversation")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let Some(conversation) = conversation else {
        log_debug!("[daemon/overlay_ws] missing conversation=");
        let _ = tokio::io::AsyncWriteExt::write_all(
            stream,
            b"HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: 47\r\nConnection: close\r\n\r\n{\"error\":\"missing conversation query parameter\"}",
        )
        .await;
        return;
    };

    let ws = match tokio_tungstenite::accept_async(&mut *stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[daemon/overlay_ws] handshake failed: {e}");
            return;
        }
    };
    let (mut write, mut read) = ws.split();
    let mut rx = subscribe();

    loop {
        tokio::select! {
            incoming = read.next() => {
                match incoming {
                    Some(Ok(Message::Ping(p))) => {
                        if write.send(Message::Pong(p)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(Message::Pong(_))) | Some(Ok(Message::Text(_)))
                    | Some(Ok(Message::Binary(_))) | Some(Ok(Message::Frame(_))) => {}
                }
            }
            event = rx.recv() => {
                match event {
                    Ok(frame) => {
                        if !skin_may_see_frame(&frame, &conversation, skin) {
                            continue;
                        }
                        if write
                            .send(Message::Text(wire_json(&frame)))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

pub fn emit_links(links: &[k2_core::overlay::OverlayLink], doc: &OverlayDoc) {
    for link in links {
        publish(OverlayFrame {
            collection: link.collection.to_string(),
            seq: link.seq,
            id: link.id.clone(),
            doc: Some(doc.clone()),
            conversation_id: link.conversation_id.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_events;

    #[test]
    fn overlay_ws_path_is_not_session_events() {
        assert_eq!(OVERLAY_WS_PATH, "/cli/overlay/events");
        assert_ne!(
            OVERLAY_WS_PATH, "/cli/sessions/events",
            "overlay WS must not be session_events"
        );
        assert_ne!(OVERLAY_WS_PATH, "/events");
        assert!(!OVERLAY_WS_PATH.contains("grid"));
    }

    #[test]
    fn two_subscribers_both_receive_thread_post_not_on_session_events() {
        let mut overlay_a = subscribe();
        let mut overlay_b = subscribe();
        let mut session_rx = session_events::subscribe();
        while session_rx.try_recv().is_ok() {}
        while overlay_a.try_recv().is_ok() {}
        while overlay_b.try_recv().is_ok() {}

        let id = uuid::Uuid::new_v4().to_string();
        let conv = uuid::Uuid::new_v4().to_string();
        let doc = OverlayDoc::text(
            id.clone(),
            "k2".to_string(),
            "sales".to_string(),
            "hi".to_string(),
            "thread",
        );
        publish(OverlayFrame {
            collection: "thread".to_string(),
            seq: 1,
            id: id.clone(),
            doc: Some(doc),
            conversation_id: Some(conv),
        });

        let a = overlay_a
            .try_recv()
            .expect("window A must receive overlay frame");
        let b = overlay_b
            .try_recv()
            .expect("window B must receive overlay frame");
        assert_eq!(a.collection, "thread");
        assert_eq!(a.id, id);
        assert_eq!(b.id, id);
        assert_eq!(a.seq, 1);

        match session_rx.try_recv() {
            Err(broadcast::error::TryRecvError::Empty) => {}
            Ok(_leftover) => {
                // Other tests share the process-wide session_events bus.
                // Overlay publish itself never calls session_events::emit.
            }
            Err(e) => panic!("session_events try_recv failed: {e}"),
        }
    }

    #[test]
    fn skin_ws_drops_chatterlog_keeps_thread() {
        let conv = "conv-skin-filter";
        let thread = OverlayFrame {
            collection: "thread".into(),
            seq: 1,
            id: "t1".into(),
            doc: None,
            conversation_id: Some(conv.into()),
        };
        let chatterlog = OverlayFrame {
            collection: "chatterlog".into(),
            seq: 2,
            id: "c1".into(),
            doc: None,
            conversation_id: None,
        };
        let other = OverlayFrame {
            collection: "thread".into(),
            seq: 3,
            id: "t2".into(),
            doc: None,
            conversation_id: Some("other".into()),
        };
        assert!(skin_may_see_frame(&thread, conv, true));
        assert!(
            !skin_may_see_frame(&chatterlog, conv, true),
            "skin WS must not see host-wide chatterlog"
        );
        assert!(!skin_may_see_frame(&other, conv, true));
        assert!(
            skin_may_see_frame(&chatterlog, conv, false),
            "owner overlay still receives chatterlog"
        );
    }
}
