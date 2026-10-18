//! `WS /cli/overlay/events?conversation=` — overlay push bus.
//!
//! Distinct from `/cli/sessions/events` (the 256-slot session_events bus)
//! and from grid-WS. Frames: `{collection, seq, id, doc?}`.
//!
//! Thread sync: every overlay write route (`thread/post|ask|secret|answer|
//! void`, the card expiry sweep, `k2 msg` chatter) publishes one
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
    // thread-read-v1: a Thread write (a post, a card's new status) may
    // change every connected person's counts for that conversation.
    if frame.collection == "thread" {
        if let Some(conv) = frame.conversation_id.as_deref() {
            crate::read_live::conversation_changed(k2_core::read_state::KIND_THREAD, conv);
        }
    }
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

/// Wire collection of the "this Thread moved" frame.
pub const MOVED_COLLECTION: &str = "moved";

/// A Thread moved to a new conversation key (Codex/Hermes adoption). One
/// `moved` frame goes to the subscribers of BOTH keys: a view on the old
/// key re-subscribes on `to` and re-reads; a view already on `to` re-reads
/// (its rows were renumbered). `seq` = Thread rows now under `to`. Wire:
/// `{collection:"moved", seq, id:<to>, from, to}`. Not stored; app guests
/// never see it (they only get `thread`, and only on a pinned Chat).
pub fn publish_moved(mv: &k2_core::overlay::ConversationMove) {
    for conv in [&mv.from, &mv.to] {
        publish(OverlayFrame {
            collection: MOVED_COLLECTION.to_string(),
            seq: mv.thread_total as i64,
            id: mv.to.clone(),
            doc: None,
            activity: Some(serde_json::json!({ "from": mv.from, "to": mv.to })),
            conversation_id: Some(conv.clone()),
        });
    }
}

/// Wire collection of the "this Thread's address changed" frame.
pub const ADDRESS_COLLECTION: &str = "address";

/// A chat was renamed, so its Thread answers at a new address (TR13).
/// Every open Thread view on the conversation (Agents, Home, Zen) swaps
/// the address it sends to and shows; items, socket and turn stay. Wire:
/// `{collection:"address", seq:0, id:<conversation>, address, previous}`.
/// Not stored; app guests never see it (thread frames only).
pub fn publish_address(conversation_id: &str, address: &str, previous: &str) {
    publish(OverlayFrame {
        collection: ADDRESS_COLLECTION.to_string(),
        seq: 0,
        id: conversation_id.to_string(),
        doc: None,
        activity: Some(serde_json::json!({ "address": address, "previous": previous })),
        conversation_id: Some(conversation_id.to_string()),
    });
}

fn on_conversation_moved(mv: &k2_core::overlay::ConversationMove) {
    log_debug!(
        "[daemon/overlay_ws] Thread moved {} -> {} ({} thread rows, {} chatter rows)",
        mv.from,
        mv.to,
        mv.thread_moved,
        mv.chatter_moved
    );
    publish_moved(mv);
    // thread-read-v1: the cursors were remapped in the move; every
    // connected person gets the new key's counts.
    crate::read_live::conversation_changed(k2_core::read_state::KIND_THREAD, &mv.to);
    // The open Thread turn follows its conversation. Off this thread: the
    // caller holds the shared DB lock, and the tracker must never be taken
    // under it.
    let (from, to) = (mv.from.clone(), mv.to.clone());
    let spawned = std::thread::Builder::new()
        .name("thread-activity-move".into())
        .spawn(move || crate::thread_activity::conversation_moved(&from, &to));
    if let Err(e) = spawned {
        log_debug!("[daemon/overlay_ws] could not move the Thread turn: {e}");
    }
}

/// Register the k2-core overlay move listener (idempotent). Called at boot
/// and by the test harness.
pub fn install_move_listener() {
    k2_core::overlay::set_move_listener(on_conversation_moved);
}

/// A guest subscriber's room (thread-attachments-v1): its pass and the
/// room it subscribed in, for the per-attachment file-rights check.
pub(crate) struct GuestWire {
    pub pass: SkinPass,
    pub project_id: String,
    pub root: Option<std::path::PathBuf>,
}

/// [`wire_json`] for one subscriber. A doc with attachments is rewritten:
/// each gains `state: "ok"` (frames go out at post time; clients refresh
/// state on the next GET), and a guest sees only the files its own file
/// rights cover (the rest are `{kind, state:"hidden"}`).
pub(crate) fn wire_json_for(frame: &OverlayFrame, guest: Option<&GuestWire>) -> String {
    let has_files = frame.doc.as_ref().is_some_and(|d| !d.attachments.is_empty());
    if !has_files || frame.activity.is_some() {
        return wire_json(frame);
    }
    let mut doc = serde_json::to_value(frame.doc.as_ref()).unwrap_or(serde_json::Value::Null);
    match guest {
        Some(g) => crate::thread_attachments::rewrite_doc(
            &mut doc,
            g.root.as_deref(),
            &crate::thread_attachments::Viewer::Guest { pass: &g.pass, project_id: &g.project_id },
            false,
        ),
        None => crate::thread_attachments::rewrite_doc(&mut doc, None, &crate::thread_attachments::Viewer::Full, false),
    }
    serde_json::json!({
        "collection": frame.collection,
        "seq": frame.seq,
        "id": frame.id,
        "doc": doc,
    })
    .to_string()
}

fn wire_json(frame: &OverlayFrame) -> String {
    if frame.collection == ADDRESS_COLLECTION {
        let payload = frame.activity.as_ref();
        return serde_json::json!({
            "collection": frame.collection,
            "seq": frame.seq,
            "id": frame.id,
            "address": payload.and_then(|p| p.get("address")).cloned().unwrap_or(serde_json::Value::Null),
            "previous": payload.and_then(|p| p.get("previous")).cloned().unwrap_or(serde_json::Value::Null),
        })
        .to_string();
    }
    if frame.collection == MOVED_COLLECTION {
        let payload = frame.activity.as_ref();
        return serde_json::json!({
            "collection": frame.collection,
            "seq": frame.seq,
            "id": frame.id,
            "from": payload.and_then(|p| p.get("from")).cloned().unwrap_or(serde_json::Value::Null),
            "to": frame.id,
        })
        .to_string();
    }
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
) -> Result<String, crate::cli_response::CliResponse> {
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
        Ok(project_id)
    } else {
        Err(crate::skin_routes::skin_room_response())
    }
}

/// WS handler. Dispatcher already token-authed. Requires `conversation=`.
/// Skin: pass `Some(SkinPass)` so rooms + pinned-Chat are checked **before**
/// `accept_async`. Owner/Connect: `None`.
///
/// `viewer_login` (Thread push S4, TP16): the socket's login as the
/// dispatcher resolved it (owner token → `owner`, a Connect session → its
/// username; `None` for app guests). Its `{"t":"viewing","visible":bool}`
/// text frames feed `thread_viewers` (state with a 90 s TTL, no ack); the
/// socket closing clears them.
pub async fn serve_overlay_events_connection(
    stream: &mut TcpStream,
    params: HashMap<String, String>,
    skin_pass: Option<SkinPass>,
    viewer_login: Option<String>,
) {
    let skin = skin_pass.is_some();
    let conversation = params
        .get("conversation")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let Some(conversation) = conversation else {
        log_debug!("[daemon/overlay_ws] missing conversation=");
        let body = r#"{"error":"missing conversation query parameter"}"#;
        let resp = format!(
            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{frame}Connection: close\r\n\r\n{body}",
            body.len(),
            frame = k2_core::frame_policy::CONNECT_HEADER_LINES,
        );
        let _ = tokio::io::AsyncWriteExt::write_all(stream, resp.as_bytes()).await;
        return;
    };

    let mut guest_wire: Option<GuestWire> = None;
    if let Some(ref pass) = skin_pass {
        match skin_overlay_gate(pass, &conversation) {
            Ok(project_id) => {
                let root = crate::thread_attachments::project_root(&project_id);
                guest_wire = Some(GuestWire { pass: pass.clone(), project_id, root });
            }
            Err(r) => {
                let resp = format!(
                    "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n{}",
                    r.status,
                    r.content_type,
                    r.body.len(),
                    k2_core::frame_policy::CONNECT_HEADER_LINES,
                    r.body
                );
                let _ = tokio::io::AsyncWriteExt::write_all(stream, resp.as_bytes()).await;
                return;
            }
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
    let socket_id = crate::thread_viewers::next_socket_id();
    let viewer_login = if skin { None } else { viewer_login };
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
                    Some(Ok(Message::Text(text))) => {
                        // Thread push S4: `viewing` frames (fire-and-forget).
                        if let (Some(login), Some(visible)) =
                            (viewer_login.clone(), crate::thread_viewers::parse_viewing(&text))
                        {
                            let conv = conversation.clone();
                            let _ = tokio::task::spawn_blocking(move || {
                                crate::thread_viewers::on_frame(socket_id, &login, &conv, visible)
                            })
                            .await;
                        }
                    }
                    Some(Ok(Message::Pong(_)))
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
                            .send(Message::Text(wire_json_for(&frame, guest_wire.as_ref())))
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
    crate::thread_viewers::global().close_socket(socket_id);
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

        // The overlay bus is process-wide: a parallel test may publish
        // between our drain and our publish (soak 2026-10-14, seed 7). Skip
        // other tests' frames; ours must be on each receiver, published
        // synchronously above, so an empty receiver fails at once.
        let own_frame = |rx: &mut broadcast::Receiver<OverlayFrame>, window: &str| loop {
            match rx.try_recv() {
                Ok(f) if f.id == id => break f,
                Ok(_other_test) => continue,
                Err(e) => panic!("window {window} must receive the overlay frame: {e}"),
            }
        };
        let a = own_frame(&mut overlay_a, "A");
        let b = own_frame(&mut overlay_b, "B");
        assert_eq!(a.collection, "thread");
        assert_eq!(b.collection, "thread");
        assert_eq!(a.seq, 1);
        assert_eq!(b.seq, 1);

        // Other tests share the process-wide session_events bus, so leftovers
        // may be there; none of them may carry our overlay doc.
        loop {
            match session_rx.try_recv() {
                Ok(ev) => {
                    let wire = serde_json::to_string(&ev).expect("session event serializes");
                    assert!(!wire.contains(&id), "overlay publish leaked onto session_events: {wire}");
                }
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(e) => panic!("session_events try_recv failed: {e}"),
            }
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
