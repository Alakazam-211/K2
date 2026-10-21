//! `WS /cli/thread/stream?rooms=all|<id>,<id>` — one live Thread stream
//! across every room a caller may read (app-room-stream, K2 0.46.3).
//! Contract: `.k2/prds/contract-app-room-stream-0463.md`.
//!
//! Beside the per-room overlay socket (`/cli/overlay/events?conversation=`),
//! which is unchanged. One socket carries the pinned-Chat Thread frames of
//! a SET of rooms, in the Companion inbox stream's frame format
//! (`contract-phone-inbox-stream-0462.md` §2: `inbox_subscribed`,
//! `thread_message` with `conversation_id`/`collection`/`seq`/`id`/`rev`,
//! `inbox_reset`), plus `room` (the room's project id) on every frame.
//!
//! **Who:** an App pass (`k2skn_`: a guest session, a platform token, an
//! App's link pass), through the App helper, the App's gateway link or the
//! main port; or a login (owner token / Connect session), which is how a
//! Garden widget streams the person's own rooms. Agent passports never.
//!
//! **Security equals the per-room socket** (`overlay_ws::skin_overlay_gate`
//! + `skin_may_see_frame` + `wire_json_for`):
//! - A guest's set is only rooms on its pass with `thread:read` in that
//!   room, and only each room's pinned Chat. Every frame is re-checked
//!   against a FRESHLY resolved pass before it is sent (a revoked room, a
//!   revoked pass, a suspended App: the next frame never goes out), and
//!   the set is recomputed every [`RECHECK`] (grants and revokes show up
//!   as a new `inbox_subscribed`; a dead pass closes 4003).
//! - Only `thread` frames reach a guest (no chatter, chatterlog, activity,
//!   moved or address frames); the doc is `wire_json_for` with the
//!   guest's room, so attachments outside its folders are `hidden`.
//! - R3 invisibility: nothing about rooms the pass can't read. Requested
//!   ids that aren't readable come back in `unknown` whether they exist or
//!   not; a guest's `rev` is this socket's own counter (C4: never the
//!   server-wide write counter) and there is no `instanceId`.
//! - On an App's gateway link the socket holds 1 live subscription (the
//!   gate's) plus 1 per room in its set (`apps::quotas`); a set over the
//!   App's quota is cut and answered with `inbox_error
//!   too_many_subscriptions`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

use k2_core::log_debug;
use k2_core::skin::SkinPass;

use crate::apps::gateway::LinkCtx;
use crate::overlay_ws::{self, GuestWire, OverlayFrame};

/// Wire path. Under `/cli/thread/` so the Direct front door's existing
/// `/cli/thread/*` matcher already reaches it.
pub const ROOM_STREAM_WS_PATH: &str = "/cli/thread/stream";
/// The socket's required query parameter (GA1): `all` or a comma list.
pub const ROOMS_PARAM: &str = "rooms";
/// Most rooms one socket streams (the inbox stream's 200).
pub const MAX_ROOMS: usize = 200;
/// Largest serialized `thread_message` (the inbox stream's 32 KiB).
pub const FRAME_CAP_BYTES: usize = 32 * 1024;
/// How often the room set is recomputed (grants, revokes, a new pinned
/// Chat). Frames are re-checked one by one regardless.
pub const RECHECK: Duration = Duration::from_secs(1);

/// What the client asked to stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Want {
    /// Every readable room (rooms granted later join live).
    All,
    /// These rooms (project ids) or their pinned conversations.
    Some(Vec<String>),
    /// Nothing (the socket stays open).
    Off,
}

/// Who holds the socket.
pub enum Viewer {
    /// An App pass, as presented (re-resolved on every frame and tick).
    Guest { raw: String },
    /// The owner token or a Connect login (reads every Thread, as on
    /// `GET /cli/thread`).
    Login,
}

/// The rooms a caller may stream now: room (project id) → its pinned
/// conversation.
pub type Readable = BTreeMap<String, String>;

/// The socket's current set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct View {
    /// room → conversation, at most [`MAX_ROOMS`].
    pub rooms: Readable,
    /// Requested ids that aren't readable (same answer whether or not
    /// they exist).
    pub unknown: Vec<String>,
}

impl View {
    fn room_of(&self, conversation: &str) -> Option<&str> {
        self.rooms.iter().find(|(_, c)| c.as_str() == conversation).map(|(r, _)| r.as_str())
    }
}

/// `rooms=` → [`Want`]. `all`, or 1–200 comma-separated ids.
pub fn parse_rooms_param(raw: &str) -> Result<Want, String> {
    let raw = raw.trim();
    if raw.eq_ignore_ascii_case("all") {
        return Ok(Want::All);
    }
    let ids: Vec<String> = raw.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
    want_from_ids(ids)
}

fn want_from_ids(ids: Vec<String>) -> Result<Want, String> {
    let mut out: Vec<String> = Vec::new();
    for id in ids {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    if out.is_empty() {
        return Err("rooms= is `all` or a comma-separated list of room ids".into());
    }
    if out.len() > MAX_ROOMS {
        return Err(format!("at most {MAX_ROOMS} rooms per socket, got {}", out.len()));
    }
    Ok(Want::Some(out))
}

/// A client text frame. `None` = not a subscribe message (ignored).
/// `{"t":"inbox","all":true}` · `{"t":"inbox","rooms":[..],
/// "conversationIds":[..]}` · `{"t":"inbox","off":true}`.
pub fn parse_subscribe(text: &str) -> Option<Result<Want, (&'static str, String)>> {
    let v: Value = serde_json::from_str(text).ok()?;
    if v.get("t").and_then(Value::as_str) != Some("inbox") {
        return None;
    }
    if v.get("off") == Some(&Value::Bool(true)) {
        return Some(Ok(Want::Off));
    }
    if v.get("all") == Some(&Value::Bool(true)) {
        return Some(Ok(Want::All));
    }
    let mut ids = Vec::new();
    for key in ["rooms", "conversationIds"] {
        match v.get(key) {
            None | Some(Value::Null) => {}
            Some(Value::Array(a)) => {
                for x in a {
                    match x.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                        Some(s) => ids.push(s.to_string()),
                        None => return Some(Err(("usage", format!("{key} holds non-empty strings")))),
                    }
                }
            }
            Some(_) => return Some(Err(("usage", format!("{key} is an array of strings")))),
        }
    }
    if ids.len() > MAX_ROOMS {
        return Some(Err(("too_many", format!("at most {MAX_ROOMS} rooms per socket, got {}", ids.len()))));
    }
    Some(want_from_ids(ids).map_err(|e| ("usage", e)))
}

/// The set for `want` over what the caller may read now.
pub fn compute_view(readable: &Readable, want: &Want) -> View {
    match want {
        Want::Off => View::default(),
        Want::All => View { rooms: readable.iter().take(MAX_ROOMS).map(|(r, c)| (r.clone(), c.clone())).collect(), unknown: Vec::new() },
        Want::Some(ids) => {
            let mut v = View::default();
            for id in ids {
                let hit = readable.get_key_value(id).or_else(|| readable.iter().find(|(_, c)| *c == id));
                match hit {
                    Some((r, c)) => {
                        v.rooms.insert(r.clone(), c.clone());
                    }
                    None => v.unknown.push(id.clone()),
                }
            }
            v
        }
    }
}

fn pinned(conn: &rusqlite::Connection, project_id: &str) -> Option<String> {
    k2_core::db::schema::WorkspaceSession::get(conn, project_id)
        .ok()
        .flatten()
        .and_then(|s| s.session_id)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// A pass's rooms with `thread:read` there and a pinned Chat (the same
/// three checks as `skin_overlay_gate`, per room).
pub fn readable_for_pass(conn: &rusqlite::Connection, pass: &SkinPass) -> Readable {
    let rooms: Vec<String> = if pass.session { pass.room_policy.keys().cloned().collect() } else { pass.rooms.clone() };
    let mut out = Readable::new();
    for room in rooms {
        let room = room.trim().to_string();
        if room.is_empty() || !pass.has_room(&room) || !pass.has_cap_in_room(&room, crate::skin_routes::THREAD_READ) {
            continue;
        }
        if let Some(conv) = pinned(conn, &room) {
            out.insert(room, conv);
        }
    }
    out
}

/// A login reads every Thread: every workspace with a pinned Chat.
pub fn readable_for_login(conn: &rusqlite::Connection) -> Readable {
    let mut out = Readable::new();
    fn pins(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<(String, String)>> {
        let mut st = conn.prepare("SELECT project_id, session_id FROM workspace_sessions WHERE session_id IS NOT NULL")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.collect()
    }
    match pins(conn) {
        Ok(rows) => {
            for (room, conv) in rows {
                let conv = conv.trim().to_string();
                if !conv.is_empty() {
                    out.insert(room, conv);
                }
            }
        }
        Err(e) => log_debug!("[daemon/room_stream] login rooms: {e}"),
    }
    out
}

/// One `thread_message`, capped at [`FRAME_CAP_BYTES`]: over the cap the
/// body goes (`truncated`, `bodyBytes`), then the whole doc. `conversation_id`,
/// `room`, `collection`, `seq`, `id` and `rev` are always there.
pub fn thread_message(conversation: &str, room: &str, seq: i64, id: &str, rev: u64, doc: Value) -> String {
    let mut v = json!({
        "kind": "thread_message",
        "conversation_id": conversation,
        // The App surface's name for it (`conversation_id` is a banned
        // field in exposed shapes, UWA6); the inbox stream's key stays.
        "conversation": conversation,
        "room": room,
        "collection": "thread",
        "seq": seq,
        "id": id,
        "rev": rev,
        "doc": doc,
    });
    let text = v.to_string();
    if text.len() <= FRAME_CAP_BYTES {
        return text;
    }
    let body_bytes = v["doc"]["body"].as_str().map(str::len);
    if let (Some(n), Some(doc)) = (body_bytes, v["doc"].as_object_mut()) {
        doc.remove("body");
        v["truncated"] = json!(true);
        v["bodyBytes"] = json!(n);
        let text = v.to_string();
        if text.len() <= FRAME_CAP_BYTES {
            return text;
        }
    }
    if let Some(m) = v.as_object_mut() {
        m.remove("doc");
    }
    v["truncated"] = json!(true);
    v.to_string()
}

/// The doc an overlay frame carries for this viewer (`wire_json_for`, the
/// per-room socket's own builder).
fn doc_for(frame: &OverlayFrame, guest: Option<&GuestWire>) -> Value {
    serde_json::from_str::<Value>(&overlay_ws::wire_json_for(frame, guest))
        .ok()
        .and_then(|mut v| v.get_mut("doc").map(Value::take))
        .unwrap_or(Value::Null)
}

fn subscribed_json(view: &View, want: &Want, rev: u64, guest: bool) -> String {
    let resolved: serde_json::Map<String, Value> = view.rooms.iter().map(|(r, c)| (r.clone(), json!(c))).collect();
    let mut v = json!({
        "kind": "inbox_subscribed",
        "rev": rev,
        "revScope": if guest { "socket" } else { "server" },
        "all": *want == Want::All,
        "rooms": view.rooms.keys().collect::<Vec<_>>(),
        "conversations": view.rooms.values().collect::<Vec<_>>(),
        "resolved": resolved,
        "unknown": view.unknown,
        "max": MAX_ROOMS,
    });
    if !guest {
        v["instanceId"] = json!(crate::boot_status::instance_id());
    }
    v.to_string()
}

fn error_json(code: &str, hint: &str, limit: Option<u32>) -> String {
    let mut v = json!({ "kind": "inbox_error", "code": code, "hint": hint });
    if let Some(l) = limit {
        v["limit"] = json!(l);
    }
    v.to_string()
}

// ── quotas on an App's gateway link ──────────────────────────────────

/// The per-room subscriptions this socket holds on its App's link (the
/// gate already counted the socket itself). Released on drop.
struct RoomSlots {
    app_id: Option<String>,
    held: usize,
}

impl RoomSlots {
    /// Hold exactly `want` slots; `Err(cap)` when only fewer fit (then
    /// that many are held).
    fn resize(&mut self, want: usize) -> Result<(), u32> {
        let Some(app) = self.app_id.clone() else {
            return Ok(());
        };
        while self.held > want {
            crate::apps::quotas::unsubscribe(&app);
            self.held -= 1;
        }
        while self.held < want {
            if !crate::apps::quotas::subscribe(&app) {
                return Err(crate::apps::quotas::quotas_for(&app).subscriptions);
            }
            self.held += 1;
        }
        Ok(())
    }
}

impl Drop for RoomSlots {
    fn drop(&mut self) {
        let _ = self.resize(0);
    }
}

// ── the socket ───────────────────────────────────────────────────────

/// Recompute the readable rooms. `None` = the guest's pass no longer
/// resolves (revoked, expired, App suspended).
fn readable_now(viewer: &Viewer, link: Option<&LinkCtx>) -> Option<(Readable, Option<SkinPass>)> {
    match viewer {
        Viewer::Login => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            Some((readable_for_login(&conn), None))
        }
        Viewer::Guest { raw } => {
            let pass = crate::apps::gateway::resolve_pass_on(link, raw)?;
            let db = k2_core::db::shared();
            let conn = db.lock();
            let r = readable_for_pass(&conn, &pass);
            Some((r, Some(pass)))
        }
    }
}

/// One guest frame, re-checked against the pass as it is NOW: `Ok(None)`
/// = the room is no longer readable (or the conversation no longer its
/// pinned Chat); `Err(())` = the pass is gone.
fn guest_doc(raw: &str, link: Option<&LinkCtx>, frame: &OverlayFrame, room: &str) -> Result<Option<Value>, ()> {
    let pass = crate::apps::gateway::resolve_pass_on(link, raw).ok_or(())?;
    if !pass.has_room(room) || !pass.has_cap_in_room(room, crate::skin_routes::THREAD_READ) {
        return Ok(None);
    }
    // An action (settings) card is an owner/admin ask: never a guest's.
    if frame.doc.as_ref().is_some_and(|d| d.is_action_card()) {
        return Ok(None);
    }
    let conv = frame.conversation_id.as_deref().unwrap_or_default();
    let still_pinned = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        pinned(&conn, room).is_some_and(|p| p == conv)
    };
    if !still_pinned {
        return Ok(None);
    }
    let root = crate::thread_attachments::project_root(room);
    let wire = GuestWire { pass, project_id: room.to_string(), root };
    Ok(Some(doc_for(frame, Some(&wire))))
}

async fn send_http(stream: &mut TcpStream, status: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n{body}",
        body.len(),
        k2_core::frame_policy::CONNECT_HEADER_LINES,
    );
    let _ = tokio::io::AsyncWriteExt::write_all(stream, resp.as_bytes()).await;
}

/// WS handler. The dispatcher has authenticated `viewer` (a guest pass
/// that resolved with `thread:read` admitted at the door, or `token_ok`).
pub async fn serve_room_stream_connection(stream: &mut TcpStream, params: HashMap<String, String>, viewer: Viewer) {
    let guest = matches!(viewer, Viewer::Guest { .. });
    let mut want = match params.get(ROOMS_PARAM).map(|s| parse_rooms_param(s)) {
        Some(Ok(w)) => w,
        Some(Err(e)) => {
            send_http(stream, "400 Bad Request", &json!({ "error": "usage", "hint": e }).to_string()).await;
            return;
        }
        None => {
            send_http(stream, "400 Bad Request", r#"{"error":"usage","hint":"missing rooms= (all, or a comma-separated list of room ids)"}"#).await;
            return;
        }
    };
    let link: Option<Arc<LinkCtx>> = crate::apps::gateway::current();
    let Some((readable, _)) = readable_now(&viewer, link.as_deref()) else {
        let r = crate::skin_routes::revoked_skin_response();
        send_http(stream, r.status, &r.body).await;
        return;
    };

    let ws = match tokio_tungstenite::accept_async(&mut *stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[daemon/room_stream] handshake failed: {e}");
            return;
        }
    };
    let (mut write, mut read) = ws.split();
    let mut rx = overlay_ws::subscribe();
    let mut slots = RoomSlots { app_id: link.as_ref().map(|l| l.app_id.clone()), held: 0 };
    // A guest's `rev`: this socket's own counter (C4).
    let mut socket_rev: u64 = 0;
    let mut view = View::default();
    let mut keepalive = tokio::time::interval(overlay_ws::ping_interval());
    keepalive.tick().await;
    let mut recheck = tokio::time::interval(RECHECK);
    recheck.tick().await;

    // Apply a fresh readable set to `want`; send `inbox_subscribed` when
    // the set changed or `announce` (an answer to a subscribe message).
    macro_rules! apply {
        ($readable:expr, $announce:expr) => {{
            let mut next = compute_view(&$readable, &want);
            let mut over: Option<u32> = None;
            if let Err(cap) = slots.resize(next.rooms.len()) {
                // Keep the rooms that fit, in room order.
                let keep = slots.held;
                next.rooms = next.rooms.into_iter().take(keep).collect();
                over = Some(cap);
            }
            let changed = next != view;
            view = next;
            let mut ok = true;
            // Said once per change (not on every recheck while it lasts).
            if let (Some(cap), true) = (over, changed || $announce) {
                let hint = format!("this App holds at most {cap} live subscriptions (the socket counts 1 plus 1 per room)");
                ok &= write.send(Message::Text(error_json("too_many_subscriptions", &hint, Some(cap)))).await.is_ok();
            }
            if ok && (changed || $announce) {
                let rev = if guest { socket_rev } else { overlay_ws::current_rev() };
                ok &= write.send(Message::Text(subscribed_json(&view, &want, rev, guest))).await.is_ok();
            }
            ok
        }};
    }

    if !apply!(readable, true) {
        return;
    }

    loop {
        tokio::select! {
            biased;
            _ = keepalive.tick() => {
                if write.send(Message::Ping(Vec::new())).await.is_err() {
                    break;
                }
            }
            _ = recheck.tick() => {
                match readable_now(&viewer, link.as_deref()) {
                    Some((r, _)) => {
                        if !apply!(r, false) {
                            break;
                        }
                    }
                    None => {
                        let _ = write.send(crate::presence::session_revoked_close_message()).await;
                        break;
                    }
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
                    Some(Ok(Message::Text(text))) => match parse_subscribe(&text) {
                        None => {}
                        Some(Err((code, hint))) => {
                            let limit = (code == "too_many").then_some(MAX_ROOMS as u32);
                            if write.send(Message::Text(error_json(code, &hint, limit))).await.is_err() {
                                break;
                            }
                        }
                        Some(Ok(next)) => {
                            want = next;
                            match readable_now(&viewer, link.as_deref()) {
                                Some((r, _)) => {
                                    if !apply!(r, true) {
                                        break;
                                    }
                                }
                                None => {
                                    let _ = write.send(crate::presence::session_revoked_close_message()).await;
                                    break;
                                }
                            }
                        }
                    },
                    Some(Ok(_)) => {}
                }
            }
            event = rx.recv() => {
                let frame = match event {
                    Ok(f) => f,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // Not a close (that would cost the whole set): drop
                        // the backlog, keep the set, tell the client to
                        // catch up (`POST /cli/thread/catchup`).
                        log_debug!("[daemon/room_stream] lagged {n} frames; inbox_reset");
                        rx = rx.resubscribe();
                        if write.send(Message::Text(r#"{"kind":"inbox_reset","reason":"lagged"}"#.into())).await.is_err() {
                            break;
                        }
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let Some(conv) = frame.conversation_id.clone() else { continue };
                let Some(room) = view.room_of(&conv).map(str::to_string) else { continue };
                let out = match (&viewer, frame.collection.as_str()) {
                    (Viewer::Login, "thread") => {
                        let doc = doc_for(&frame, None);
                        Some(thread_message(&conv, &room, frame.seq, &frame.id, frame.rev, doc))
                    }
                    (Viewer::Login, overlay_ws::MOVED_COLLECTION) => {
                        // Published under both keys; the set holds `from`.
                        let from = frame.activity.as_ref().and_then(|a| a.get("from")).and_then(Value::as_str).unwrap_or_default();
                        (from == conv).then(|| {
                            json!({ "kind": "thread_moved", "room": room, "from": from, "to": frame.id, "seq": frame.seq, "rev": frame.rev }).to_string()
                        })
                    }
                    (Viewer::Login, overlay_ws::ADDRESS_COLLECTION) => {
                        let a = frame.activity.clone().unwrap_or(Value::Null);
                        Some(json!({ "kind": "thread_address", "room": room, "conversation_id": conv, "address": a["address"], "previous": a["previous"], "rev": frame.rev }).to_string())
                    }
                    (Viewer::Guest { raw }, "thread") => {
                        let (raw, link2, f, r) = (raw.clone(), link.clone(), frame.clone(), room.clone());
                        let checked = tokio::task::spawn_blocking(move || guest_doc(&raw, link2.as_deref(), &f, &r)).await;
                        match checked {
                            Ok(Ok(Some(doc))) => {
                                socket_rev += 1;
                                Some(thread_message(&conv, &room, frame.seq, &frame.id, socket_rev, doc))
                            }
                            Ok(Ok(None)) => {
                                // Revoked (or a new pinned Chat): never sent;
                                // the set follows now, not at the next tick.
                                match readable_now(&viewer, link.as_deref()) {
                                    Some((r, _)) => {
                                        if !apply!(r, false) {
                                            break;
                                        }
                                    }
                                    None => {
                                        let _ = write.send(crate::presence::session_revoked_close_message()).await;
                                        break;
                                    }
                                }
                                None
                            }
                            Ok(Err(())) => {
                                let _ = write.send(crate::presence::session_revoked_close_message()).await;
                                break;
                            }
                            Err(e) => {
                                log_debug!("[daemon/room_stream] frame check panicked: {e}");
                                break;
                            }
                        }
                    }
                    // A guest gets only `thread` frames (as on the per-room
                    // socket); a move re-resolves the set at the next tick.
                    _ => None,
                };
                if let Some(text) = out {
                    if write.send(Message::Text(text)).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
    drop(slots);
}

#[cfg(test)]
mod tests;
