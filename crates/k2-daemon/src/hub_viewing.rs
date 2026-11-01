//! Viewing on the session-events hub (hub-viewing-v1, 0.46.8;
//! `.k2/prds/vslive-phone-speed-b1-b2-rev-v1.md` B1 + hello rev).
//!
//! K2 Companion holds ONE `/cli/sessions/events` socket. While a chat is
//! open on screen it sends, on that socket,
//! ```json
//! {"t":"viewing","conversationId":"c-1","visible":true}
//! ```
//! every 30 s, and `visible:false` when the chat leaves the screen. The
//! daemon then:
//! - registers the view in the per-socket `thread_viewers` registry (the
//!   one the overlay socket's `viewing` frames feed), under this socket's
//!   own slot ([`Slot`], from `thread_viewers::next_socket_id`, never the
//!   hub's `subscriber_id`), so Thread pushes to that person stay quiet
//!   exactly as they do for an open overlay socket. 90 s TTL, refreshed by
//!   a repeat; `visible:false` or the socket closing clears it at once;
//! - answers a NEW view with `viewing_ok` (`conversation_id`, `rev`,
//!   `convRev`, `instanceId`, `turn`). `rev` is the overlay bus high-water
//!   mark, read after the view is registered and the bus receivers are
//!   taken; `convRev` is the last `rev` published for this chat
//!   (`overlay_ws::conv_rev`); `turn` is what `GET /cli/thread/activity`
//!   would return. Phone rule: skip the reopen read when `instanceId`
//!   matches and `convRev <= lastApplied`. A repeat of the same chat sends
//!   nothing;
//! - forwards, while the view lasts, the chat's `thread_message`,
//!   `thread_moved` and `thread_address` (once each, even when the chat is
//!   not in the socket's inbox set: one classify, one send) and its turn
//!   frames as `thread_activity` (a lag sends `thread_activity_reset`, the
//!   turn as it is now). A move moves the view.
//!
//! **One view per socket:** a new `visible:true` for another chat clears
//! the old one first. Repeats of the same chat within [`COALESCE`] don't
//! touch the registry (each touch reads the people roster from disk).
//!
//! **Who.** Only a login reaches the hub (the owner token or a Connect
//! session; App passes and agent passports get 403 before the upgrade).
//! A login reads every Thread (`GET /cli/thread`'s rule), so a view adds no
//! access; the one Thread a login can't read is a Grok Bot's while Local
//! Grok Bots' Show is off ([`authorize`]). A refusal is a `viewing_error`
//! frame, never a silent ack. App guests keep the overlay socket or
//! `push.viewing`.
//!
//! Memory only (no migration). After a restart the registry is empty; the
//! phone re-sends `viewing` on reopen and the new `instanceId` forces its
//! read.

use std::time::Duration;

use futures_util::SinkExt;
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

use k2_core::log_debug;

use crate::inbox_stream::{self, Hit, InboxSet};
use crate::overlay_ws::{self, OverlayFrame};
use crate::thread_viewers;

/// A `viewing` frame longer than this is ignored.
pub const MAX_FRAME_BYTES: usize = 4096;

/// Longest `conversationId` (chars).
pub const MAX_ID_CHARS: usize = 128;

/// Repeats of the same chat closer than this don't touch the registry.
pub const COALESCE: Duration = Duration::from_secs(5);

/// The frame kinds this module sends (not `SessionEvent` variants; the
/// shared registry lists them with class `viewing`). The chat's
/// `thread_message`, `thread_moved` and `thread_address` are the inbox
/// stream's kinds.
pub const VIEWING_KINDS: &[&str] = &["viewing_ok", "viewing_error", "thread_activity", "thread_activity_reset"];

/// One client `viewing` frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewFrame {
    pub conversation: String,
    pub visible: bool,
}

/// A client text frame. `None` = not a `viewing` frame (or over
/// [`MAX_FRAME_BYTES`]): ignored, as every other client text on this socket.
pub fn parse(text: &str) -> Option<Result<ViewFrame, (&'static str, String)>> {
    if text.len() > MAX_FRAME_BYTES {
        return None;
    }
    let v: Value = serde_json::from_str(text).ok()?;
    if v.get("t").and_then(Value::as_str) != Some("viewing") {
        return None;
    }
    let bad_id = || Some(Err(("usage", format!("conversationId is a string of 1 to {MAX_ID_CHARS} characters"))));
    let conversation = match v.get("conversationId") {
        Some(Value::String(s)) => s.trim(),
        _ => return bad_id(),
    };
    let n = conversation.chars().count();
    if n == 0 || n > MAX_ID_CHARS {
        return bad_id();
    }
    let Some(visible) = v.get("visible").and_then(Value::as_bool) else {
        return Some(Err(("usage", "visible is true or false".into())));
    };
    Some(Ok(ViewFrame { conversation: conversation.to_string(), visible }))
}

/// May this socket's login view `conversation`? The same rule as reading
/// that Thread: every login reads every Thread, except a Grok Bot's while
/// Show is off. An unresolved login (`None`) views nothing.
pub fn authorize(viewer_login: Option<&str>, conversation: &str) -> Result<(), (&'static str, String)> {
    if viewer_login.is_none() {
        return Err(("unauthorized", "this socket's login could not be resolved; reconnect".into()));
    }
    let grokbot = k2_core::external_agents::collect::conversation_id("");
    if conversation.to_ascii_lowercase().starts_with(&grokbot) && !k2_core::external_agents::settings::load().show {
        return Err(("forbidden", "Local Grok Bots are off on this server, so their Threads can't be viewed".into()));
    }
    Ok(())
}

/// `viewing_ok`. `reason` is set only on a repeat answer the daemon sends
/// by itself (`lagged`: the socket fell behind the overlay bus and the
/// view's frames may be lost; compare `convRev` again).
pub fn ok_json(conversation: &str, opened: &Opened, reason: Option<&str>) -> String {
    let mut v = json!({
        "kind": "viewing_ok",
        "conversation_id": conversation,
        "rev": opened.rev,
        "convRev": opened.conv_rev,
        "instanceId": crate::boot_status::instance_id(),
        "turn": opened.turn,
    });
    if let Some(r) = reason {
        v["reason"] = json!(r);
    }
    v.to_string()
}

/// `viewing_error` (`usage` | `unauthorized` | `forbidden` | `internal`).
pub fn error_json(code: &str, hint: &str, conversation: Option<&str>) -> String {
    let mut v = json!({ "kind": "viewing_error", "code": code, "hint": hint });
    if let Some(c) = conversation {
        v["conversation_id"] = json!(c);
    }
    v.to_string()
}

/// One turn frame of the viewed chat (§7.6 shape, plus `kind` and
/// `conversation_id`).
pub fn activity_json(conversation: &str, frame: &OverlayFrame) -> String {
    json!({
        "kind": "thread_activity",
        "conversation_id": conversation,
        "collection": "activity",
        "seq": 0,
        "id": frame.id,
        "activity": frame.activity,
    })
    .to_string()
}

/// The turn as it is now, after turn frames were lost (`null` = none
/// running). The phone replaces its turn with it.
pub fn activity_reset_json(conversation: &str, snapshot: Option<&Value>) -> String {
    json!({
        "kind": "thread_activity_reset",
        "conversation_id": conversation,
        "collection": overlay_ws::ACTIVITY_RESET_COLLECTION,
        "seq": 0,
        "id": snapshot.and_then(|s| s.get("turnId")).cloned().unwrap_or(Value::Null),
        "activity": snapshot.cloned().unwrap_or(Value::Null),
    })
    .to_string()
}

/// This socket's slot in the `thread_viewers` registry. Dropping it (any
/// exit path of the socket loop) clears every view it held.
#[derive(Debug)]
pub struct Slot(pub u64);

impl Slot {
    pub fn new() -> Self {
        Self(thread_viewers::next_socket_id())
    }
}

impl Default for Slot {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        thread_viewers::global().close_socket(self.0);
    }
}

/// What `viewing_ok` carries.
#[derive(Debug, Clone, PartialEq)]
pub struct Opened {
    pub rev: u64,
    pub conv_rev: u64,
    pub turn: Option<Value>,
}

/// The socket's one view.
#[derive(Debug, Clone)]
pub struct HubView {
    pub conversation: String,
    /// The view as a one-chat set, so the inbox `classify` serves both.
    set: InboxSet,
    /// Frames at or below this `rev` were already in the client's read
    /// (the `rev` its `viewing_ok` gave).
    pub since_rev: u64,
    /// When the registry was last touched for it.
    applied_at: Instant,
}

impl HubView {
    pub fn new(conversation: &str, since_rev: u64, now: Instant) -> Self {
        Self {
            conversation: conversation.to_string(),
            set: InboxSet { ids: vec![conversation.to_string()], ..Default::default() },
            since_rev,
            applied_at: now,
        }
    }

    /// When the view lapses without a repeat (the registry's TTL).
    pub fn until(&self) -> Instant {
        self.applied_at + thread_viewers::VIEW_TTL
    }

    /// A repeat `visible:true` for this chat at `now`: `true` when the
    /// registry must be touched (outside [`COALESCE`]).
    pub fn repeat(&mut self, now: Instant) -> bool {
        if now.saturating_duration_since(self.applied_at) < COALESCE {
            return false;
        }
        self.applied_at = now;
        true
    }

    /// What an overlay frame means for the view (frames already covered by
    /// its `viewing_ok` never count).
    pub fn hit(&self, frame: &OverlayFrame) -> Option<Hit> {
        if frame.rev <= self.since_rev {
            return None;
        }
        inbox_stream::classify(frame, &self.set)
    }

    /// A Thread moved `from` → `to`. `Some((old, new))` when the viewed
    /// chat itself moved (the registry follows it).
    pub fn apply_move(&mut self, from: &str, to: &str) -> Option<(String, String)> {
        if self.conversation != from {
            return None;
        }
        self.set.apply_move(from, to);
        self.conversation = to.to_string();
        Some((from.to_string(), to.to_string()))
    }

    /// Is this turn frame the viewed chat's?
    pub fn owns_activity(&self, frame: &OverlayFrame) -> bool {
        frame.conversation_id.as_deref() == Some(self.conversation.as_str())
    }
}

/// The socket's viewing state: its registry slot, its view, and the turn
/// receiver (taken only while a view lasts).
#[derive(Debug, Default)]
pub struct ViewState {
    pub slot: Slot,
    pub view: Option<HubView>,
    pub activity_rx: Option<broadcast::Receiver<OverlayFrame>>,
}

/// `rev`, `convRev` and the turn, read in that order (the caller has
/// already registered the view and taken its receivers).
pub fn snapshot_blocking(conversation: &str) -> Opened {
    let (rev, conv_rev) = overlay_ws::rev_snapshot(conversation);
    let turn = crate::thread_activity::current_turn(conversation);
    Opened { rev, conv_rev, turn }
}

/// Register `conversation` for `slot` (clearing `old` first), then
/// [`snapshot_blocking`].
fn open_blocking(slot: u64, login: &str, old: Option<&str>, conversation: &str) -> Opened {
    if let Some(old) = old.filter(|o| *o != conversation) {
        thread_viewers::on_frame(slot, login, old, false);
    }
    thread_viewers::on_frame(slot, login, conversation, true);
    snapshot_blocking(conversation)
}

/// Touch the registry off the async worker (`on_frame` reads the people
/// roster from disk).
async fn touch(slot: u64, login: Option<&str>, conversation: &str, visible: bool) {
    let Some(login) = login.map(str::to_string) else { return };
    let conv = conversation.to_string();
    if let Err(e) = tokio::task::spawn_blocking(move || thread_viewers::on_frame(slot, &login, &conv, visible)).await {
        log_debug!("[daemon/hub_viewing] registry update failed: {e}");
    }
}

/// End the view (`visible:false`, or its TTL lapsed): the registry entry
/// goes, and so do the receivers nothing else needs.
pub async fn end(state: &mut ViewState, overlay_rx: &mut Option<broadcast::Receiver<OverlayFrame>>, inbox_active: bool, login: Option<&str>) {
    let Some(v) = state.view.take() else { return };
    state.activity_rx = None;
    if !inbox_active {
        *overlay_rx = None;
    }
    touch(state.slot.0, login, &v.conversation, false).await;
}

/// The viewed chat moved `old` → `new`: the registry follows it.
pub async fn follow_move(state: &ViewState, login: Option<&str>, old: &str, new: &str) {
    touch(state.slot.0, login, old, false).await;
    touch(state.slot.0, login, new, true).await;
}

/// Send one text frame; `Err` = the socket is gone.
async fn send<W>(write: &mut W, text: String) -> Result<(), ()>
where
    W: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    write.send(Message::Text(text)).await.map_err(|e| {
        log_debug!("[daemon/hub_viewing] send failed: {e}");
    })
}

/// Apply one client `viewing` frame. `Err` = the socket is gone (break).
pub async fn on_frame<W>(
    write: &mut W,
    state: &mut ViewState,
    overlay_rx: &mut Option<broadcast::Receiver<OverlayFrame>>,
    inbox_active: bool,
    login: Option<&str>,
    frame: ViewFrame,
) -> Result<(), ()>
where
    W: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let now = Instant::now();
    let conv = frame.conversation;
    let current = state.view.as_ref().map(|v| v.conversation.clone());
    if !frame.visible {
        // Only the socket's own view ends; anything else is already off.
        if current.as_deref() == Some(conv.as_str()) {
            end(state, overlay_rx, inbox_active, login).await;
        }
        return Ok(());
    }
    if current.as_deref() == Some(conv.as_str()) {
        // A repeat: refresh the TTL (coalesced), send nothing.
        let refresh = state.view.as_mut().is_some_and(|v| v.repeat(now));
        if refresh {
            touch(state.slot.0, login, &conv, true).await;
        }
        return Ok(());
    }
    // A new chat on screen: the old one is no longer looked at, whether or
    // not this one may be viewed.
    let (who, c) = (login.map(str::to_string), conv.clone());
    let auth = tokio::task::spawn_blocking(move || authorize(who.as_deref(), &c)).await;
    let refusal = match auth {
        Ok(Ok(())) => None,
        Ok(Err((code, hint))) => Some((code, hint)),
        Err(e) => {
            log_debug!("[daemon/hub_viewing] authorize failed: {e}");
            Some(("internal", "the view could not be checked; send it again".to_string()))
        }
    };
    if let Some((code, hint)) = refusal {
        end(state, overlay_rx, inbox_active, login).await;
        return send(write, error_json(code, &hint, Some(&conv))).await;
    }
    let Some(who) = login.map(str::to_string) else {
        // `authorize` refuses a missing login; never registered.
        return send(write, error_json("unauthorized", "no login", Some(&conv))).await;
    };
    // Receivers first (a fresh turn receiver: nothing of the old chat's
    // queued), then register, then `rev`, `convRev`, the turn.
    state.activity_rx = Some(overlay_ws::subscribe_activity());
    if overlay_rx.is_none() {
        *overlay_rx = Some(overlay_ws::subscribe());
    }
    let old = state.view.take().map(|v| v.conversation);
    let slot = state.slot.0;
    let c = conv.clone();
    let opened = tokio::task::spawn_blocking(move || open_blocking(slot, &who, old.as_deref(), &c)).await;
    match opened {
        Ok(opened) => {
            state.view = Some(HubView::new(&conv, opened.rev, now));
            send(write, ok_json(&conv, &opened, None)).await
        }
        Err(e) => {
            log_debug!("[daemon/hub_viewing] open failed: {e}");
            state.activity_rx = None;
            if !inbox_active {
                *overlay_rx = None;
            }
            // The slot may hold the half-registered view: clear it.
            thread_viewers::global().close_socket(slot);
            send(write, error_json("internal", "the view could not be opened; send it again", Some(&conv))).await
        }
    }
}

#[cfg(test)]
mod tests;
