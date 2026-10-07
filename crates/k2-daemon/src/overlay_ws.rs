//! `WS /cli/overlay/events?conversation=` — overlay push bus.
//!
//! Distinct from `/cli/sessions/events` (the 256-slot session_events bus)
//! and from grid-WS. Frames: `{collection, seq, id, doc?}`.
//!
//! Thread sync: every overlay write route (`thread/post|ask|secret|answer|
//! void`, compose prose that voids cards, `k2 msg` chatter) publishes one
//! frame here, and every client view of a Thread subscribes and merges by
//! id. This socket is the only push path, so it must not die quietly:
//!
//! - the server sends a WS Ping every [`ping_interval`] (20 s), so an idle
//!   Thread is not cut by the tunnel edge (HAProxy `timeout tunnel 1h`, the
//!   web edge's idle cut) and a dead peer is found by the failed write;
//! - a subscriber that lags the bus is CLOSED, never silently skipped, so
//!   the client reconnects and catches up with `GET /cli/thread?since_seq=`.
//!
//! Thread working strip (prd-daemon-activity-and-thread-working-v1 TW5,
//! A25): `collection: "activity"` frames are ephemeral (never stored, no
//! seq) and ride a SECOND broadcast ([`publish_activity`]), merged into
//! each socket's `select!` after the Thread bus. A lag on that bus is
//! skipped, not closed: `GET /cli/thread/activity` is the catch-up, and
//! strip traffic must never force a Thread resync. App guests never see
//! them ([`skin_may_see_frame`] passes only `thread`).

use std::collections::HashMap;
use std::sync::OnceLock;

use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

use k2_core::log_debug;
use k2_core::overlay::OverlayDoc;
use k2_core::skin::SkinPass;

/// Wire path. Tests assert this is not `session_events`.
pub const OVERLAY_WS_PATH: &str = "/cli/overlay/events";

const BUS_CAP: usize = 1024;

/// The activity bus (A25). Strip frames are at most 2/s per turn; a
/// subscriber that falls this far behind skips.
const ACTIVITY_BUS_CAP: usize = 256;

/// Keepalive ping cadence: 20 s by default. `K2_OVERLAY_WS_PING_MS`
/// overrides it (the headless test uses a short one).
pub fn ping_interval() -> std::time::Duration {
    let ms = std::env::var("K2_OVERLAY_WS_PING_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(20_000);
    std::time::Duration::from_millis(ms)
}

#[derive(Debug, Clone, Serialize)]
pub struct OverlayFrame {
    pub collection: String,
    pub seq: i64,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<OverlayDoc>,
    /// TW5 / §7.6: the Thread working strip's turn (`collection:
    /// "activity"` only). Never stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity: Option<serde_json::Value>,
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

fn activity_bus() -> &'static broadcast::Sender<OverlayFrame> {
    static TX: OnceLock<broadcast::Sender<OverlayFrame>> = OnceLock::new();
    TX.get_or_init(|| broadcast::channel(ACTIVITY_BUS_CAP).0)
}

/// TW5 / A25: publish an ephemeral `activity` frame. Never stored, no
/// seq, its own bus (a lag there skips instead of closing the socket).
pub fn publish_activity(frame: OverlayFrame) {
    let _ = activity_bus().send(frame);
}

pub fn subscribe_activity() -> broadcast::Receiver<OverlayFrame> {
    activity_bus().subscribe()
}

fn wire_json(frame: &OverlayFrame) -> String {
    match &frame.activity {
        // §7.6: an activity frame has no `doc`.
        Some(activity) => serde_json::json!({
            "collection": frame.collection,
            "seq": frame.seq,
            "id": frame.id,
            "activity": activity,
        }),
        None => serde_json::json!({
            "collection": frame.collection,
            "seq": frame.seq,
            "id": frame.id,
            "doc": frame.doc,
        }),
    }
    .to_string()
}

/// Whether this overlay subscriber may see `frame`.
///
/// Skin WS is Thread-only: host-wide chatterlog (`conversation_id: None`)
/// AND per-conversation `chatter` (agent-to-agent `k2 msg`, same cid as
/// the pinned Chat) are dropped. HTTP `/cli/chatter` is already 403 for
/// skins. Owner/Connect subscribers keep the unfiltered bus.
pub fn skin_may_see_frame(frame: &OverlayFrame, conversation: &str, skin: bool) -> bool {
    match &frame.conversation_id {
        Some(cid) => cid == conversation && (!skin || frame.collection == "thread"),
        None => !skin, // chatterlog is host-wide — never a skin room
    }
}

fn skin_overlay_gate(
    pass: &SkinPass,
    conversation: &str,
) -> Result<(), crate::cli_response::CliResponse> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Ok(Some(project_id)) =
        k2_core::workspace_session_handles::project_id_for_session_id(&conn, conversation)
    else {
        return Err(crate::skin_routes::skin_room_response());
    };
    if !pass.has_room(&project_id) {
        return Err(crate::skin_routes::skin_room_response());
    }
    if !pass.has_cap_in_room(&project_id, crate::skin_routes::THREAD_READ) {
        return Err(crate::skin_routes::missing_cap_response(
            crate::skin_routes::THREAD_READ,
        ));
    }
    let Ok(Some(session)) = k2_core::db::schema::WorkspaceSession::get(&conn, &project_id) else {
        return Err(crate::skin_routes::skin_room_response());
    };
    let pinned = session
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_some_and(|pin| pin == conversation);
    if pinned {
        Ok(())
    } else {
        Err(crate::skin_routes::skin_room_response())
    }
}

/// WS handler. Dispatcher already token-authed. Requires `conversation=`.
/// Skin: pass `Some(SkinPass)` so rooms + pinned-Chat are checked **before**
/// `accept_async`. Owner/Connect: `None`.
pub async fn serve_overlay_events_connection(
    stream: &mut TcpStream,
    params: HashMap<String, String>,
    skin_pass: Option<SkinPass>,
) {
    let skin = skin_pass.is_some();
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

    if let Some(ref pass) = skin_pass {
        if let Err(r) = skin_overlay_gate(pass, &conversation) {
            let resp = format!(
                "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                r.status,
                r.content_type,
                r.body.len(),
                r.body
            );
            let _ = tokio::io::AsyncWriteExt::write_all(stream, resp.as_bytes()).await;
            return;
        }
    }

    let ws = match tokio_tungstenite::accept_async(&mut *stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[daemon/overlay_ws] handshake failed: {e}");
            return;
        }
    };
    let (mut write, mut read) = ws.split();
    let mut rx = subscribe();
    let mut activity_rx = subscribe_activity();
    let mut keepalive = tokio::time::interval(ping_interval());
    keepalive.tick().await; // burn the immediate first tick

    loop {
        tokio::select! {
            // A Thread message goes out before a strip frame that is ready
            // at the same time (a compose post publishes both).
            biased;
            _ = keepalive.tick() => {
                if write.send(Message::Ping(Vec::new())).await.is_err() {
                    break;
                }
            }
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
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // Frames were dropped for this subscriber. Skipping
                        // them would leave its Thread silently behind; close
                        // so the client reconnects and catches up by seq.
                        log_debug!(
                            "[daemon/overlay_ws] subscriber lagged {n} frames; closing for resync"
                        );
                        let _ = write.send(Message::Close(None)).await;
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            event = activity_rx.recv() => {
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
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // A25: strip frames are ephemeral and the client
                        // re-reads the current turn with GET
                        // /cli/thread/activity; skipping is safe, closing
                        // would force a Thread resync for nothing.
                        log_debug!("[daemon/overlay_ws] activity subscriber lagged {n} frames; skipped");
                    }
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
            activity: None,
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
            activity: None,
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
            activity: None,
            conversation_id: Some(conv.into()),
        };
        let chatterlog = OverlayFrame {
            collection: "chatterlog".into(),
            seq: 2,
            id: "c1".into(),
            doc: None,
            activity: None,
            conversation_id: None,
        };
        let other = OverlayFrame {
            collection: "thread".into(),
            seq: 3,
            id: "t2".into(),
            doc: None,
            activity: None,
            conversation_id: Some("other".into()),
        };
        let chatter_same = OverlayFrame {
            collection: "chatter".into(),
            seq: 4,
            id: "ch1".into(),
            doc: None,
            activity: None,
            conversation_id: Some(conv.into()),
        };
        assert!(skin_may_see_frame(&thread, conv, true));
        assert!(
            !skin_may_see_frame(&chatterlog, conv, true),
            "skin WS must not see host-wide chatterlog"
        );
        assert!(!skin_may_see_frame(&other, conv, true));
        assert!(
            !skin_may_see_frame(&chatter_same, conv, true),
            "skin WS must not see agent-to-agent chatter on the pinned conversation"
        );
        assert!(
            skin_may_see_frame(&chatterlog, conv, false),
            "owner overlay still receives chatterlog"
        );
        assert!(
            skin_may_see_frame(&chatter_same, conv, false),
            "owner overlay still receives per-conversation chatter"
        );
    }

    /// TW5 / AP1: the Thread working strip's `activity` frames (tool
    /// lines, thinking) never reach an app guest; owners on the same
    /// conversation get them, owners elsewhere don't.
    #[test]
    fn skin_ws_drops_activity_frames() {
        let conv = "conv-activity-filter";
        let activity = OverlayFrame {
            collection: "activity".into(),
            seq: 0,
            id: "turn-1".into(),
            doc: None,
            activity: Some(serde_json::json!({ "turnId": "turn-1", "line": "Running `cargo test`" })),
            conversation_id: Some(conv.into()),
        };
        assert!(!skin_may_see_frame(&activity, conv, true), "an app guest must never see an activity frame");
        assert!(skin_may_see_frame(&activity, conv, false));
        assert!(!skin_may_see_frame(&activity, "another-conv", false));
    }

    /// §7.6: an activity frame carries `activity`, no `doc`, seq 0.
    #[test]
    fn activity_wire_shape_has_no_doc() {
        let frame = OverlayFrame {
            collection: "activity".into(),
            seq: 0,
            id: "turn-2".into(),
            doc: None,
            activity: Some(serde_json::json!({ "turnId": "turn-2" })),
            conversation_id: Some("c".into()),
        };
        let v: serde_json::Value = serde_json::from_str(&wire_json(&frame)).expect("json");
        assert_eq!(v["collection"], "activity");
        assert_eq!(v["seq"], 0);
        assert_eq!(v["id"], "turn-2");
        assert_eq!(v["activity"]["turnId"], "turn-2");
        assert!(v.get("doc").is_none(), "{v}");
        assert!(v.get("conversation_id").is_none(), "internal key leaked: {v}");
        let thread = OverlayFrame { collection: "thread".into(), activity: None, ..frame };
        let v: serde_json::Value = serde_json::from_str(&wire_json(&thread)).expect("json");
        assert!(v.get("activity").is_none(), "{v}");
        assert!(v.get("doc").is_some(), "thread frames keep `doc` (null): {v}");
    }
}
