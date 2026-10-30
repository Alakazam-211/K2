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
//!
//! App guests' ear (0.46.3, Rosson 2026-10-21): the tracker also publishes
//! each frame's counts-only guest projection on a THIRD bus
//! ([`publish_guest_activity`]); an owner frame never goes to a guest. A
//! guest socket sends it as the same `{collection:"activity", seq:0, id,
//! activity}` shape only when the client opted in on the URL (`?ear=1`,
//! [`EAR_PARAM`]: a live legacy App reloads its Thread on any frame, so
//! without the opt-in a guest gets exactly the r5 frames) and only while
//! [`guest_ear_allowed`] holds for the pass as it is NOW (re-resolved per frame: a revoked pass closes the socket, a
//! revoked cap or room sends nothing): the room on the pass,
//! `thread:read` AND `activity:read` there, this conversation its pinned
//! Chat.
//!
//! Guest read frames (app-read-state-v1, 0.46.3): an App guest's socket
//! also gets `{"collection":"read",…}` ([`guest_read_wire`]) when its own
//! reader's cursor or mute moves in this Thread on any surface
//! (`read_live::guest_moved`); owner/Connect sockets never do (they read
//! `thread_read` on `/cli/sessions/events`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// inbox-catchup-v1 (§3.1): the per-process write counter, stamped by
    /// [`publish`] (whatever the caller set is overwritten). `0` = not
    /// published on the Thread bus (activity frames). It goes on the wire
    /// as an extra `rev` key; never a cursor, never persisted, restarts
    /// with the daemon (pair it with `boot_status::instance_id()`).
    #[serde(skip)]
    pub rev: u64,
}

/// The last `rev` stamped by [`publish`] (0 before the first frame).
static REV: AtomicU64 = AtomicU64::new(0);

/// Stamp + send happen under this lock, so bus order is `rev` order.
static PUBLISH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// inbox-catchup-v1: the overlay bus high-water mark. A Thread read takes
/// it BEFORE its store read. Frames are published only after their write
/// commits, so any frame with `rev <=` this value is already in the read,
/// and every later frame has a higher `rev`.
pub fn current_rev() -> u64 {
    REV.load(Ordering::SeqCst)
}

fn bus() -> &'static broadcast::Sender<OverlayFrame> {
    static TX: OnceLock<broadcast::Sender<OverlayFrame>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, _rx) = broadcast::channel(BUS_CAP);
        tx
    })
}

pub fn publish(mut frame: OverlayFrame) {
    // thread-read-v1: a Thread write (a post, a card's new status) may
    // change every connected person's counts for that conversation.
    if frame.collection == "thread" {
        if let Some(conv) = frame.conversation_id.as_deref() {
            crate::read_live::conversation_changed(k2_core::read_state::KIND_THREAD, conv);
        }
        // push-content-available-v1: a person's post or a closed card
        // alerts nobody; their phones get a stale hint instead.
        crate::stale_hint::on_thread_frame(&frame);
    }
    let _guard = PUBLISH_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    frame.rev = REV.fetch_add(1, Ordering::SeqCst) + 1;
    let _ = bus().send(frame);
}

/// catchup-unchanged-v1: a Thread change that has NO frame of its own (a
/// Thread import, a collected outside-agent conversation such as `gb:`)
/// still moves `rev`, so a catch-up `sinceRev` taken before it is stale
/// and the next catch-up reads. `rev` is a high-water mark: it may skip
/// numbers on the wire (never a cursor, never contiguous). Returns the new
/// `rev`.
pub fn bump_rev(reason: &str, conversation_id: &str) -> u64 {
    let _guard = PUBLISH_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let rev = REV.fetch_add(1, Ordering::SeqCst) + 1;
    #[cfg(test)]
    test_bumps::record(reason, conversation_id, rev);
    #[cfg(not(test))]
    let _ = (reason, conversation_id);
    rev
}

/// [`bump_rev`] for every conversation a Thread import wrote lines into.
pub fn note_import(report: &k2_core::overlay::transfer::ImportReport) {
    for c in &report.conversations {
        if c.imported > 0 {
            bump_rev("import", &c.local_conversation_id);
        }
    }
}

/// Test-only record of [`bump_rev`] calls (process-global; tests filter by
/// their own conversation ids).
#[cfg(test)]
pub(crate) mod test_bumps {
    use std::sync::Mutex;
    static BUMPS: Mutex<Vec<(String, String, u64)>> = Mutex::new(Vec::new());

    pub(crate) fn record(reason: &str, conversation_id: &str, rev: u64) {
        BUMPS.lock().unwrap_or_else(|p| p.into_inner()).push((reason.to_string(), conversation_id.to_string(), rev));
    }

    /// The `(reason, rev)` bumps recorded for one conversation.
    pub(crate) fn for_conversation(conversation_id: &str) -> Vec<(String, u64)> {
        BUMPS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter(|(_, c, _)| c == conversation_id)
            .map(|(r, _, v)| (r.clone(), *v))
            .collect()
    }
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

/// The per-connection opt-in for the guest ear (0.46.3): `?ear=1` on
/// `/cli/overlay/events` and `/cli/thread/stream`. The App SDK's
/// `thread.events` / `thread.stream` helpers set it (the catalog row's
/// socket `query`). The catch-up GET is a pull and needs none.
pub const EAR_PARAM: &str = "ear";

/// Did this socket ask for the ear (`ear=1` / `ear=true`)?
pub fn ear_opted_in(params: &HashMap<String, String>) -> bool {
    params.get(EAR_PARAM).is_some_and(|v| matches!(v.trim(), "1" | "true"))
}

/// One Thread turn frame's guest projection (0.46.3,
/// `thread_activity::guest_turn_body`). Never the owner body.
#[derive(Debug, Clone, PartialEq)]
pub struct GuestTurnFrame {
    pub conversation_id: String,
    pub turn_id: String,
    pub activity: serde_json::Value,
}

fn guest_activity_bus() -> &'static broadcast::Sender<GuestTurnFrame> {
    static TX: OnceLock<broadcast::Sender<GuestTurnFrame>> = OnceLock::new();
    TX.get_or_init(|| broadcast::channel(ACTIVITY_BUS_CAP).0)
}

/// Publish a guest turn frame (the tracker, beside [`publish_activity`]).
pub fn publish_guest_activity(frame: GuestTurnFrame) {
    let _ = guest_activity_bus().send(frame);
}

pub fn subscribe_guest_activity() -> broadcast::Receiver<GuestTurnFrame> {
    guest_activity_bus().subscribe()
}

/// The guest turn frame on the per-room socket: the §7.6 shape, the guest
/// body in `activity`, no `doc`, no `rev`.
pub fn guest_activity_wire(frame: &GuestTurnFrame) -> String {
    serde_json::json!({
        "collection": "activity",
        "seq": 0,
        "id": frame.turn_id,
        "activity": frame.activity,
    })
    .to_string()
}

/// The reset frame's collection on the per-room socket (0.46.5).
pub const ACTIVITY_RESET_COLLECTION: &str = "activity_reset";

/// 0.46.5 (review B1): the socket fell behind on an activity bus and
/// frames were dropped. Instead of skipping silently (a lost end frame
/// left the bar stuck), the socket gets this: the conversation's turn as
/// it is NOW (`activity`: the owner body, or a guest's projection), or
/// `null` when none is running. A client replaces its turn with it.
pub fn activity_reset_wire(snapshot: Option<&serde_json::Value>) -> String {
    serde_json::json!({
        "collection": ACTIVITY_RESET_COLLECTION,
        "seq": 0,
        "id": snapshot.and_then(|s| s.get("turnId")).cloned().unwrap_or(serde_json::Value::Null),
        "activity": snapshot.cloned().unwrap_or(serde_json::Value::Null),
    })
    .to_string()
}

/// 0.46.3: may `pass` hear the counts-only ear of `conversation` in
/// `room`, NOW? The room on the pass, `thread:read` AND `activity:read` in
/// that room, and `conversation` is the room's pinned Chat. Every guest
/// door of the ear (this socket, the room stream, `GET
/// /cli/thread/activity`) asks this with a freshly resolved pass.
pub(crate) fn guest_ear_allowed(pass: &SkinPass, room: &str, conversation: &str) -> bool {
    if room.trim().is_empty()
        || !pass.has_room(room)
        || !pass.has_cap_in_room(room, crate::skin_routes::THREAD_READ)
        || !pass.has_cap_in_room(room, crate::skin_routes::ACTIVITY_READ)
    {
        return false;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::WorkspaceSession::get(&conn, room)
        .ok()
        .flatten()
        .and_then(|s| s.session_id)
        .is_some_and(|pin| pin.trim() == conversation)
}

/// What a guest socket does with one guest turn frame, after re-resolving
/// its pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EarCheck {
    Send,
    /// Not (or no longer) allowed: nothing is sent.
    Skip,
    /// The pass no longer resolves: close the socket.
    PassGone,
}

/// [`guest_ear_allowed`] for a raw pass as it resolves NOW (`link`: the
/// App link the socket arrived on, if any).
pub(crate) fn guest_ear_check(link: Option<&crate::apps::gateway::LinkCtx>, raw: &str, room: &str, conversation: &str) -> EarCheck {
    match crate::apps::gateway::resolve_pass_on(link, raw) {
        None => EarCheck::PassGone,
        Some(pass) if guest_ear_allowed(&pass, room, conversation) => EarCheck::Send,
        Some(_) => EarCheck::Skip,
    }
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
            rev: 0,
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
        rev: 0,
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
    /// 0.46.5 (item 14): the room's folder → its name, for every doc frame.
    pub scrub: crate::guest_paths::GuestScrub,
}

impl GuestWire {
    /// A guest subscriber in `project_id` (reads the database: the room's
    /// folder and name).
    pub(crate) fn for_room(pass: SkinPass, project_id: String) -> Self {
        let root = crate::thread_attachments::project_root(&project_id);
        let scrub = crate::guest_paths::GuestScrub::for_room(&project_id);
        Self { pass, project_id, root, scrub }
    }
}

/// [`wire_json`] for one subscriber. A doc with attachments is rewritten:
/// each gains `state: "ok"` (frames go out at post time; clients refresh
/// state on the next GET), and a guest sees only the files its own file
/// rights cover (the rest are `{kind, state:"hidden"}`).
///
/// A permission card (prd-agent-settings-cards-v1 rev 3.1 §4.2, C9) is
/// projected for `seen`'s viewer first: same id, same seq, the `cards=2`
/// view for a person who asked on the connect URL, else the fallback.
/// Projecting may read the database (the live Settings check, cached
/// 2 s): async callers run it on the blocking pool ([`needs_blocking`]).
pub(crate) fn wire_json_for(frame: &OverlayFrame, guest: Option<&GuestWire>, seen: &crate::permission::view::Seen) -> String {
    if let Some(doc) = frame.doc.as_ref().filter(|d| crate::permission::view::is_permission_card(d)) {
        let projected = OverlayFrame { doc: Some(seen.doc(doc).into_owned()), ..frame.clone() };
        return wire_json_projected(&projected, guest);
    }
    wire_json_projected(frame, guest)
}

/// True when [`wire_json_for`] would project this frame (it may then read
/// the database).
pub(crate) fn needs_blocking(frame: &OverlayFrame) -> bool {
    frame.doc.as_ref().is_some_and(crate::permission::view::is_permission_card)
}

/// This socket's viewer (rev 3.1 §17.1): the dispatcher's login (owner
/// token → `owner`, a Connect session → its username), an App guest, or
/// nobody we can name; `cards=` on the connect URL.
pub(crate) fn socket_seen(params: &HashMap<String, String>, skin: bool, viewer_login: Option<&str>) -> crate::permission::view::Seen {
    let viewer = match (skin, viewer_login) {
        (true, _) => k2_core::permission::card::Viewer::Guest { app: None },
        (false, Some(login)) => crate::permission::view::viewer_of_login(login),
        (false, None) => crate::permission::view::unknown(),
    };
    crate::permission::view::Seen::new(viewer, crate::permission::view::cards_param(params))
}

fn wire_json_projected(frame: &OverlayFrame, guest: Option<&GuestWire>) -> String {
    // inbox-catchup-v1 C4 (0.46.3): a guest's frames never carry `rev` (a
    // server-wide write counter would show how busy other rooms are).
    if guest.is_some() && frame.rev > 0 {
        let unstamped = OverlayFrame { rev: 0, ..frame.clone() };
        return wire_json_projected(&unstamped, guest);
    }
    let out = wire_json_files(frame, guest);
    // 0.46.5 (item 14): a guest's doc frames never carry a host path.
    match guest {
        Some(g) if frame.doc.is_some() => g.scrub.frame_json(out),
        _ => out,
    }
}

fn wire_json_files(frame: &OverlayFrame, guest: Option<&GuestWire>) -> String {
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
    with_rev(
        serde_json::json!({
            "collection": frame.collection,
            "seq": frame.seq,
            "id": frame.id,
            "doc": doc,
        }),
        frame,
    )
}

/// inbox-catchup-v1: a published frame carries its `rev` as an extra key
/// (current clients ignore it). Activity frames (`rev` 0) don't.
fn with_rev(mut v: serde_json::Value, frame: &OverlayFrame) -> String {
    if frame.rev > 0 {
        v["rev"] = serde_json::json!(frame.rev);
    }
    v.to_string()
}

fn wire_json(frame: &OverlayFrame) -> String {
    let payload = frame.activity.as_ref();
    let v = if frame.collection == ADDRESS_COLLECTION {
        serde_json::json!({
            "collection": frame.collection,
            "seq": frame.seq,
            "id": frame.id,
            "address": payload.and_then(|p| p.get("address")).cloned().unwrap_or(serde_json::Value::Null),
            "previous": payload.and_then(|p| p.get("previous")).cloned().unwrap_or(serde_json::Value::Null),
        })
    } else if frame.collection == MOVED_COLLECTION {
        serde_json::json!({
            "collection": frame.collection,
            "seq": frame.seq,
            "id": frame.id,
            "from": payload.and_then(|p| p.get("from")).cloned().unwrap_or(serde_json::Value::Null),
            "to": frame.id,
        })
    } else {
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
    };
    with_rev(v, frame)
}

/// Whether this overlay subscriber may see `frame`.
///
/// Skin WS is Thread-only: host-wide chatterlog (`conversation_id: None`)
/// AND per-conversation `chatter` (agent-to-agent `k2 msg`, same cid as
/// the pinned Chat) are dropped. HTTP `/cli/chatter` is already 403 for
/// skins. Owner/Connect subscribers keep the unfiltered bus.
pub fn skin_may_see_frame(frame: &OverlayFrame, conversation: &str, skin: bool) -> bool {
    // An action (settings) card is an owner/admin ask: a skin never gets
    // its frame, posted, rewritten or closed.
    if skin && frame.doc.as_ref().is_some_and(|d| d.is_action_card()) {
        return false;
    }
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

/// app-read-state-v1 (0.46.3): a guest's `read` frame on its overlay
/// socket, `{"collection":"read", id, addr, lastReadSeq, lastSeq, unread,
/// waiting, muted, mutedUntil?, total, waitingTotal}` (totals over its own
/// rooms only). Sent when the guest's own cursor or mute moved in this
/// Thread on any surface; never for another reader, never a `rev`.
pub(crate) fn guest_read_wire(mut body: serde_json::Map<String, serde_json::Value>) -> String {
    body.insert("collection".into(), serde_json::json!("read"));
    serde_json::Value::Object(body).to_string()
}

/// WS handler. Dispatcher already token-authed. Requires `conversation=`.
/// Skin: pass `Some(SkinPass)` so rooms + pinned-Chat are checked **before**
/// `accept_async`. Owner/Connect: `None`.
///
/// `viewer_login` (Thread push S4, TP16): the socket's login as the
/// dispatcher resolved it (owner token → `owner`, a Connect session → its
/// username; `None` for app guests). Its `{"t":"viewing","visible":bool}`
/// text frames feed `thread_viewers` (state with a 90 s TTL, no ack); the
/// socket closing clears them. An App guest's frames register under its
/// person key `guest:<principalId>` instead (TA-23), so the App
/// agent-reply push stays quiet while that guest looks.
/// `skin_raw` (0.46.3): the guest's raw pass, re-resolved on every guest
/// turn frame ([`guest_ear_check`]); `None` for owner/Connect. A guest
/// gets ear frames only with `?ear=1` ([`ear_opted_in`]).
use crate::routes::http::refuse_upgrade;

pub async fn serve_overlay_events_connection(
    stream: &mut TcpStream,
    params: HashMap<String, String>,
    skin_pass: Option<SkinPass>,
    skin_raw: Option<String>,
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
        refuse_upgrade(stream, &resp).await;
        return;
    };

    let mut guest_wire: Option<GuestWire> = None;
    if let Some(ref pass) = skin_pass {
        match skin_overlay_gate(pass, &conversation) {
            Ok(project_id) => {
                guest_wire = Some(GuestWire::for_room(pass.clone(), project_id));
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
                refuse_upgrade(stream, &resp).await;
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
    let seen = socket_seen(&params, skin, viewer_login.as_deref());
    let mut rx = subscribe();
    let mut activity_rx = subscribe_activity();
    // 0.46.3: a guest socket's ear is the guest bus only (never the owner
    // activity bus above, which `skin_may_see_frame` drops for it anyway).
    let mut guest_activity_rx = subscribe_guest_activity();
    let link = crate::apps::gateway::current();
    // Only a guest that opted in (`?ear=1`) has an ear room.
    let ear_room = guest_wire.as_ref().filter(|_| ear_opted_in(&params)).map(|g| g.project_id.clone());
    // app-read-state-v1 (0.46.3): a guest socket hears its own reader's
    // cursor or mute moving in this Thread, on any surface.
    let guest_key = skin_pass.as_ref().map(crate::read_routes::guest_person_key);
    // TA-23: a guest PERSON's `viewing` frames register under that key
    // (an App's own pass is not a person and never views).
    let guest_viewer_key = skin_pass
        .as_ref()
        .map(crate::read_routes::pass_reader)
        .filter(|r| r.person)
        .map(|r| r.key);
    let mut guest_rx = crate::read_live::subscribe_guest();
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
            moved = crate::read_live::next_guest_move(&mut guest_rx, guest_key.as_deref()) => {
                let (Some(pass), true) = (skin_pass.clone(), moved.conversation_id == conversation) else {
                    continue;
                };
                let conv = conversation.clone();
                let frame = tokio::task::spawn_blocking(move || crate::read_routes::guest_read_frame(&pass, &conv))
                    .await
                    .ok()
                    .flatten();
                if let Some(text) = frame.map(guest_read_wire) {
                    if write.send(Message::Text(text)).await.is_err() {
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
                    Some(Ok(Message::Text(text))) => {
                        // Thread push S4: `viewing` frames (fire-and-forget).
                        if let Some(visible) = crate::thread_viewers::parse_viewing(&text) {
                            let conv = conversation.clone();
                            if let Some(login) = viewer_login.clone() {
                                let _ = tokio::task::spawn_blocking(move || {
                                    crate::thread_viewers::on_frame(socket_id, &login, &conv, visible)
                                })
                                .await;
                            } else if let (Some(key), Some(raw)) = (guest_viewer_key.clone(), skin_raw.clone()) {
                                // TA-23: an App guest, under its person key.
                                // The pass is re-resolved per frame (as the
                                // ear is): a revoked pass registers nothing.
                                let link2 = link.clone();
                                let _ = tokio::task::spawn_blocking(move || {
                                    if crate::apps::gateway::resolve_pass_on(link2.as_deref(), &raw).is_some() {
                                        crate::thread_viewers::on_guest_frame(socket_id, &key, &conv, visible)
                                    }
                                })
                                .await;
                            }
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
                        let text = if needs_blocking(&frame) {
                            // A permission card: projected off the async
                            // worker (guests never get here: skin_may_see_frame
                            // drops every action card for them).
                            let (f, s) = (frame.clone(), seen.clone());
                            match tokio::task::spawn_blocking(move || wire_json_for(&f, None, &s)).await {
                                Ok(t) => t,
                                Err(e) => {
                                    // Never skip a frame silently: close so
                                    // the client reconnects and catches up.
                                    log_debug!("[daemon/overlay_ws] projection failed: {e}; closing for resync");
                                    let _ = write.send(Message::Close(None)).await;
                                    break;
                                }
                            }
                        } else {
                            wire_json_for(&frame, guest_wire.as_ref(), &seen)
                        };
                        if write.send(Message::Text(text)).await.is_err() {
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
            event = guest_activity_rx.recv() => {
                match event {
                    Ok(frame) => {
                        let (Some(raw), Some(room)) = (skin_raw.clone(), ear_room.clone()) else { continue };
                        if frame.conversation_id != conversation {
                            continue;
                        }
                        let (link2, conv) = (link.clone(), conversation.clone());
                        let check = tokio::task::spawn_blocking(move || guest_ear_check(link2.as_deref(), &raw, &room, &conv)).await;
                        match check {
                            Ok(EarCheck::Send) => {
                                if write.send(Message::Text(guest_activity_wire(&frame))).await.is_err() {
                                    break;
                                }
                            }
                            Ok(EarCheck::Skip) => {}
                            Ok(EarCheck::PassGone) => {
                                let _ = write.send(crate::presence::session_revoked_close_message()).await;
                                break;
                            }
                            Err(e) => {
                                log_debug!("[daemon/overlay_ws] guest ear check panicked: {e}");
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // 0.46.5 (review B1): never skip silently; a lost
                        // end would leave the guest's bar stuck. Send the
                        // turn as it is now, if the pass may still hear it.
                        log_debug!("[daemon/overlay_ws] guest activity subscriber lagged {n} frames; sending a reset");
                        let (Some(raw), Some(room)) = (skin_raw.clone(), ear_room.clone()) else { continue };
                        let (link2, conv) = (link.clone(), conversation.clone());
                        let snap = tokio::task::spawn_blocking(move || match guest_ear_check(link2.as_deref(), &raw, &room, &conv) {
                            EarCheck::Send => Ok(Some(crate::thread_activity::current_turn_guest(&conv))),
                            EarCheck::Skip => Ok(None),
                            EarCheck::PassGone => Err(()),
                        })
                        .await;
                        match snap {
                            Ok(Ok(Some(turn))) => {
                                if write.send(Message::Text(activity_reset_wire(turn.as_ref()))).await.is_err() {
                                    break;
                                }
                            }
                            Ok(Ok(None)) => {}
                            Ok(Err(())) => {
                                let _ = write.send(crate::presence::session_revoked_close_message()).await;
                                break;
                            }
                            Err(e) => {
                                log_debug!("[daemon/overlay_ws] guest reset panicked: {e}");
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            event = activity_rx.recv() => {
                match event {
                    Ok(frame) => {
                        // A guest never gets an owner strip frame (its ear
                        // is the guest bus above).
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
                        // A25: strip frames are ephemeral; closing would
                        // force a Thread resync for nothing. But skipping
                        // silently left the bar stuck when the lost frame
                        // was the end (0.46.5, review B1): send the turn as
                        // it is now instead. A guest's ear is the guest bus.
                        log_debug!("[daemon/overlay_ws] activity subscriber lagged {n} frames; sending a reset");
                        if skin {
                            continue;
                        }
                        let conv = conversation.clone();
                        match tokio::task::spawn_blocking(move || crate::thread_activity::current_turn(&conv)).await {
                            Ok(turn) => {
                                if write.send(Message::Text(activity_reset_wire(turn.as_ref()))).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                log_debug!("[daemon/overlay_ws] activity reset panicked: {e}");
                                break;
                            }
                        }
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
            rev: 0,
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

    /// 0.46.5 (review B1): the reset carries the turn as it is now, or
    /// `null` (none running), so a lost end frame can't leave a bar stuck.
    #[test]
    fn the_activity_reset_carries_the_current_turn_or_null() {
        let body = serde_json::json!({ "turnId": "t1", "state": "working", "rev": 7 });
        let v: serde_json::Value = serde_json::from_str(&activity_reset_wire(Some(&body))).expect("json");
        assert_eq!(v, serde_json::json!({ "collection": "activity_reset", "seq": 0, "id": "t1", "activity": body }));
        let v: serde_json::Value = serde_json::from_str(&activity_reset_wire(None)).expect("json");
        assert_eq!(v, serde_json::json!({ "collection": "activity_reset", "seq": 0, "id": null, "activity": null }));
    }

    /// A tokio broadcast receiver that falls behind gets `Lagged` and the
    /// lost frames are gone (the end among them). Every activity bus's
    /// lag arm, on both sockets, answers with a reset, never a bare skip.
    #[tokio::test]
    async fn every_activity_lag_sends_a_reset_never_a_silent_skip() {
        let (tx, mut rx) = broadcast::channel::<u32>(2);
        for i in 0..5 {
            tx.send(i).expect("send");
        }
        assert!(matches!(rx.recv().await, Err(broadcast::error::RecvError::Lagged(3))), "the end frame can be among the lost");
        for (file, src, arms, reset) in [
            ("overlay_ws.rs", include_str!("overlay_ws.rs"), 2, "activity_reset_wire("),
            ("room_stream_ws.rs", include_str!("room_stream_ws.rs"), 2, "activity_reset_message("),
        ] {
            let prod = src.split("\n#[cfg(test)]\nmod tests").next().expect("prod part");
            let lag_arms: Vec<&str> = prod
                .split("RecvError::Lagged(n)) =>")
                .skip(1)
                .map(|rest| &rest[..rest.find("RecvError::Closed").expect("each lag arm ends before Closed")])
                .filter(|arm| arm.contains("activity"))
                .collect();
            assert_eq!(lag_arms.len(), arms, "{file}: the activity lag arms");
            for arm in lag_arms {
                assert!(arm.contains(reset), "{file}: an activity lag arm must send a reset: {arm}");
                assert!(!arm.contains("; skipped"), "{file}: never a silent skip: {arm}");
            }
        }
    }

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
            rev: 0,
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
            rev: 0,
            collection: "thread".into(),
            seq: 1,
            id: "t1".into(),
            doc: None,
            activity: None,
            conversation_id: Some(conv.into()),
        };
        let chatterlog = OverlayFrame {
            rev: 0,
            collection: "chatterlog".into(),
            seq: 2,
            id: "c1".into(),
            doc: None,
            activity: None,
            conversation_id: None,
        };
        let other = OverlayFrame {
            rev: 0,
            collection: "thread".into(),
            seq: 3,
            id: "t2".into(),
            doc: None,
            activity: None,
            conversation_id: Some("other".into()),
        };
        let chatter_same = OverlayFrame {
            rev: 0,
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

    /// TW5 / AP1, as changed in 0.46.3 (Rosson 2026-10-21): the OWNER
    /// strip frame (tool lines, thinking) never reaches an app guest;
    /// owners on the same conversation get it, owners elsewhere don't. A
    /// guest's ear is only the guest bus ([`guest_activity_wire`], gated
    /// per frame by [`guest_ear_allowed`]).
    #[test]
    fn skin_ws_drops_owner_activity_frames() {
        let conv = "conv-activity-filter";
        let activity = OverlayFrame {
            rev: 0,
            collection: "activity".into(),
            seq: 0,
            id: "turn-1".into(),
            doc: None,
            activity: Some(serde_json::json!({ "turnId": "turn-1", "line": "Running `cargo test`" })),
            conversation_id: Some(conv.into()),
        };
        assert!(!skin_may_see_frame(&activity, conv, true), "an app guest must never see an OWNER activity frame");
        assert!(skin_may_see_frame(&activity, conv, false));
        assert!(!skin_may_see_frame(&activity, "another-conv", false));
    }

    /// 0.46.3: a guest's ear frame is the §7.6 shape with the guest body
    /// as given (built by `thread_activity::guest_turn_body`), no `doc`,
    /// no `rev`, nothing else.
    #[test]
    fn the_ear_is_opt_in_per_connection() {
        let q = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<HashMap<_, _>>();
        assert!(!ear_opted_in(&q(&[("conversation", "c")])), "no opt-in: today's frames only");
        assert!(!ear_opted_in(&q(&[("ear", "0")])));
        assert!(!ear_opted_in(&q(&[("ear", "")])));
        assert!(ear_opted_in(&q(&[("ear", "1")])));
        assert!(ear_opted_in(&q(&[("ear", "true")])));
    }

    #[test]
    fn guest_activity_wire_is_the_activity_shape_with_the_guest_body() {
        let body = crate::thread_activity::guest_turn_body(&serde_json::json!({
            "turnId": "turn-g", "state": "working", "phase": "tool", "line": "Running `cat /etc/secret`",
            "rev": 3, "serverNow": 9, "end": null,
        }));
        let frame = GuestTurnFrame { conversation_id: "c".into(), turn_id: "turn-g".into(), activity: body.clone() };
        let v: serde_json::Value = serde_json::from_str(&guest_activity_wire(&frame)).expect("json");
        let mut keys: Vec<&str> = v.as_object().expect("object").keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["activity", "collection", "id", "seq"], "{v}");
        assert_eq!((v["collection"].as_str(), v["seq"].as_i64(), v["id"].as_str()), (Some("activity"), Some(0), Some("turn-g")));
        assert_eq!(v["activity"], body);
        let text = v.to_string();
        assert!(!text.contains("secret") && !text.contains('`') && !text.contains("line"), "{text}");
    }

    /// §7.6: an activity frame carries `activity`, no `doc`, seq 0.
    #[test]
    fn activity_wire_shape_has_no_doc() {
        let frame = OverlayFrame {
            rev: 0,
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
        assert!(v.get("rev").is_none(), "an unpublished frame has no rev: {v}");
    }

    /// inbox-catchup-v1 (0.46.3): `publish` stamps a strictly rising `rev`
    /// (overwriting the caller's), `current_rev` is the high-water mark, the
    /// wire carries it for people, and a guest's frame never does (C4).
    #[test]
    fn publish_stamps_a_rising_rev_people_see_and_guests_do_not() {
        let conv = format!("rev-{}", uuid::Uuid::new_v4());
        let mut rx = subscribe();
        let before = current_rev();
        for i in 0..3 {
            publish(OverlayFrame {
                rev: 999_999,
                collection: "thread".into(),
                seq: i + 1,
                id: format!("d{i}"),
                doc: Some(OverlayDoc::text(format!("d{i}"), "k2".into(), "k2".into(), "hi".into(), "thread")),
                activity: None,
                conversation_id: Some(conv.clone()),
            });
        }
        let mut revs = Vec::new();
        while revs.len() < 3 {
            let f = rx.try_recv().expect("published frame");
            if f.conversation_id.as_deref() == Some(conv.as_str()) {
                revs.push(f.clone());
            }
        }
        let nums: Vec<u64> = revs.iter().map(|f| f.rev).collect();
        assert!(nums[0] > before && nums[0] < nums[1] && nums[1] < nums[2], "rising revs after {before}: {nums:?}");
        assert!(current_rev() >= nums[2]);
        let person: serde_json::Value = serde_json::from_str(&wire_json_for(&revs[2], None, &crate::permission::view::Seen::new(k2_core::permission::card::Viewer::OwnerToken, 0))).expect("json");
        assert_eq!(person["rev"], nums[2], "{person}");
        let pass = k2_core::skin::SkinPass {
            id: "skin-pass".into(),
            principal_id: Some("prin".into()),
            username: "guest".into(),
            caps: vec!["thread:read".into()],
            rooms: vec!["p".into()],
            session: false,
            room_policy: k2_core::skin::RoomPolicy::new(),
            app: String::new(),
            room_paths: Default::default(),
        };
        let guest = GuestWire { pass, project_id: "p".into(), root: None, scrub: Default::default() };
        let g: serde_json::Value = serde_json::from_str(&wire_json_for(&revs[2], Some(&guest), &crate::permission::view::Seen::new(k2_core::permission::card::Viewer::Guest { app: None }, 0))).expect("json");
        assert!(g.get("rev").is_none(), "a guest frame carries no rev: {g}");
        assert_eq!(g["id"], "d2", "{g}");
    }
}
