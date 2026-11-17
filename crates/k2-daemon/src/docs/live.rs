//! `WS /cli/docs/live`: the Live view's socket (plan L2). One socket is one
//! person's tab on one text doc, or (V3) one person watching a live keyed
//! doc: a deck, a timeline (`.otio`, the timeline core) or a scene
//! (`.blend`, the blend core) ([`super::live_deck`]), joined to the kit's
//! session; the editor
//! component (K2 Cores, `k2-mpc-client`) drives it through the renderer's
//! `LiveTransport` (`src/renderer/lib/docs-live-transport.ts`).
//!
//! **Who.** People only: the owner token or a Connect login, as every
//! client socket (`?token=` checked before the upgrade; the dispatcher's
//! cookie/CSRF and Origin gates run first). Agents use `k2 doc`, never this
//! socket; App guests get none in 0.46.7. Both are refused before the
//! upgrade (403). The token never reaches a log: nothing here logs the
//! query.
//!
//! **The URL.** `?doc=<path>` (absolute, or workspace-relative with
//! `&workspace=`; `path=` is the same), `&root=source` (the only text root
//! of the Markdown and text packs; a keyed doc takes it too, or its own
//! container root: `slides`, `tracks`, `colls`), `&resume=<version>` on a
//! reconnect. A keyed doc whose pack declines it at open (a timeline with
//! more than one timeline, a scene saved by a release the pack hasn't
//! checked) is `closed {reason: "declined", error}`, the kit's report with
//! the pack's code in `error.details.code`.
//!
//! **The contract is the kit's `client/WIRE.md` wire 3** (`welcome.wire`,
//! kit v0.15.x; frames in [`super::live_wire`], pinned against the kit's
//! golden JSON). K2's host events ride beside the kit's frames
//! ([`super::host_events`]): `preview_ready`, `turn_end` (with
//! `turn_idle_ms` in the welcome), and the comment relay.
//!
//! **Per socket, in order** (WIRE.md "Connecting"): resolve and jail the
//! file, check its core is on and the person may read it, `open_file`,
//! `join(Stay::Held)` (a new connection and client id, also on every
//! reconnect), `subscribe` and `subscribe_roster` **before** the read, then
//! `welcome {root, version, text?, client, self, rights}`, the first frame.
//! `text` is left out on a resume the window still covers. A text doc's
//! last outside save, when recent, follows the welcome (its `outside_save`,
//! `before_version` included: a tab that joined after the save can still
//! compare or undo it; [`super::host_events::subscribe_replaying`]). Anything refused
//! before the join is one `closed` frame and a close: no presence, no
//! session.
//!
//! **One worker thread per socket** owns the kit connection and both
//! receivers (the kit's are blocking channels). It forwards every change and
//! roster event in order, and answers the client's frames:
//! - `edit` → `edit_at_base` → version V. The changes queued before V go out
//!   first, this connection's own change at exactly V is skipped (matched by
//!   version, never by client id), then `ack {op, version, hunks}`. Nothing
//!   applied: everything queued, then `ack` at the unchanged version. A
//!   refusal: everything queued, then `refused {op, error}`.
//! - `since` → `events_since` → `events {op, events}` or `refused`.
//! - `undo` / `redo` → the change arrives as `change`, then `undone {op,
//!   steps, version, notice?}`; a conflict is `refused` (`MPC-E201`).
//! - `snapshot` → `snapshot {op, text, version}`.
//! - `cursor` → `set_cursor`, mapped to now first (`place_caret`, WIRE.md
//!   "Carets"): a caret at the doc's version goes in as it is; one behind
//!   (a change this socket had sent was still on its way to the client) is
//!   carried through `events_since` with the kit's `position::transform`,
//!   `Bias::After`. Past the live window, or at a version this socket never
//!   told the client, it is dropped (the client re-sends after its next
//!   change). No reply; a refused caret is ignored.
//! - `visible` (and the Thread's `{"t":"viewing","visible"}`) →
//!   `set_visible`; `view` → `set_view`; `hello` is ignored (the URL carries
//!   root and resume).
//! - `watch` / `report` (a deck whose welcome lists `keyed_watch`): the
//!   kit's keyed watch ([`super::live_watch`]): a snapshot in pages, then
//!   one `ops_change` per change in place of the bare `change` frames.
//!   Anywhere else, `error` with `MPC-E113`.
//! - `view_address` / `time_position` (a keyed doc's socket: its welcome
//!   lists both, kit v0.18.0) → `set_view_address` / `set_time_position`;
//!   no reply, a refusal is `presence_refused {req, error}`. A text doc's
//!   socket lists `view_address` only; `time_position` there is `error`
//!   with `MPC-E113`.
//!
//! The kit's `tick` (every 250 ms, `docs/mod.rs`) also flushes rosters, so
//! carets arrive without a timer of our own.
//!
//! **When it ends** (WIRE.md v2 "Closing"). The client closes: `leave(conn)`.
//! The kit drops the connection: `closed` with the reason (`cut_off` when the
//! subscriber fell 1,024 events behind and is still joined, `kicked`,
//! `rights_lost`, `deleted`). The doc is renamed (`k2 doc mv`): `moved`
//! with its new `path`. The daemon hands off or stops: no frame, close code
//! 1012 (the transport's `network`: reconnect to the next daemon). The
//! file's core is switched off (Settings → Cores → Multiplayer): the flip
//! closes the doc's session through the kit's host close (written back
//! first; the roster's last event is `Closed`), then [`close_core`] tells
//! each socket who did it and whether the doc is on disk, and the socket
//! sends every change still queued, then `closed {reason:"closed",
//! code:"core_off", core, version, written}` and close code 4010. A socket
//! that sees the switch off first (within a second) waits briefly for
//! that; a switch that closed nothing here makes it write back itself.
//! The person's token ends (a kick, a removal, a role change): `closed` with
//! `kicked` when the login is still on the box, else `rights_lost`, then close
//! code 4003. The worker asks before every frame that reads or writes the doc
//! and at each poll while the store moved, and on a 5 s tick for what the store
//! can't show (expiry): rights come from the login, so a token that ends leaves
//! them untouched, and the kit alone would keep this connection.
//!
//! **Keepalive** (as `overlay_ws.rs`): a WS Ping every 20 s
//! (`K2_DOCS_LIVE_PING_MS` overrides it for tests); no frame at all from the
//! client for two pings and a half closes the socket and leaves. There is
//! no app-level ping.
//!
//! **Frame cap** ([`super::live_wire::FRAME_MAX`], 4 MiB, both ways): a
//! bigger client `edit` is `refused {op}` with `MPC-E113`, any other frame
//! `error`, in order, and the socket stays (past 8 MiB the socket closes:
//! `cut_off`, 1009). Out: a `change` over the cap is withheld (the client
//! sees the gap and asks `since`), an `events` over it is `refused` with
//! `MPC-E112` (the client reloads), and a `welcome` or `snapshot` over it is
//! `closed {declined}` with `MPC-E401`.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use k2_mpc_base::errors::Value;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::Message;

use k2_core::log_debug;
use k2_mpc_base::session::position::{transform, At, Bias, LineRange};
use k2_mpc_base::session::{
    ChangeEvent, ClaimPolicy, ConnId, EditOptions, JoinOptions, RosterEvent, Session, SessionError, Stay,
};
use k2_mpc_base::store::{DocKey, Seq};
use k2_mpc_base::DocError;

use super::caller::{refusal, DocCaller};
use super::host::{self, DocRights};
use super::live_wire::{self as wire, ClientFrame, CloseReason, Closed};
use super::target::{self, Target, Tier};
use crate::cli_response::CliResponse;

/// The route. It sits under `/cli/docs/`, so the dispatcher matches it
/// before the docs family's HTTP arm.
pub const LIVE_WS_PATH: &str = "/cli/docs/live";

/// `GET /cli/docs/live/check`: the same auth and person-only checks as the
/// socket, answered over plain HTTP (204, or the socket's 403). A browser
/// can't read why an upgrade was refused, so after a failed dial (only then)
/// the transport asks this once to tell `auth` (no retry) from `network`.
/// Read-only; it never looks at the doc (`doc=` is accepted and ignored, so
/// it says nothing about which files exist).
pub const LIVE_CHECK_PATH: &str = "/cli/docs/live/check";

/// `via` on every change made through the Live view.
pub const VIA_TAB: &str = "tab";

/// The only text root a Live socket edits (Markdown and the text pack).
pub const ROOT: &str = k2_mpc_markdown::ROOT;

/// How long the worker waits for a client frame before it looks at the
/// kit's receivers again (the latency of others' changes and carets).
const POLL: Duration = Duration::from_millis(10);

/// How often the worker re-reads the core switch (the backstop when the
/// switch flips without [`close_core`]).
const GATE_EVERY: Duration = Duration::from_secs(1);

/// How long a socket that finds its core off (or its session closed by K2)
/// waits for the flip's [`close_core`], which follows the kit's close at
/// once and carries who turned it off and whether the doc was written.
const FLIP_WAIT: Duration = Duration::from_secs(2);

/// The longest the worker waits for its own change event after
/// `edit_at_base` returned (it is queued during the commit, so this is a
/// safety net: a late one is still skipped by version).
const OWN_EVENT_WAIT: Duration = Duration::from_secs(2);

/// Client frames queued for the worker. A client sends one edit at a time,
/// so a full queue is a client gone wrong: the socket is cut off.
const INBOX: usize = 256;

/// Frames queued for the socket. When the socket is this far behind, the
/// worker waits, the kit's own queue fills, and the kit cuts the subscriber
/// off (`cut_off`).
const OUTBOX: usize = 256;

/// Keepalive cadence: 20 s, `K2_DOCS_LIVE_PING_MS` overrides it.
pub fn ping_interval() -> Duration {
    let ms = std::env::var("K2_DOCS_LIVE_PING_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(20_000);
    Duration::from_millis(ms)
}

// ── The core switch (PRD multiplayer-cores-settings §2, §6.1) ─────────

/// A test's stand-in for the core switch (the integration test flips a core
/// for the Live socket alone, without a settings write or a close).
pub type CoreGate = fn(&str) -> bool;

fn gate() -> &'static RwLock<Option<CoreGate>> {
    static G: OnceLock<RwLock<Option<CoreGate>>> = OnceLock::new();
    G.get_or_init(|| RwLock::new(None))
}

/// Put a stand-in in front of the switch (`None`: the switch alone).
pub fn set_core_gate(g: Option<CoreGate>) {
    *gate().write().unwrap_or_else(|p| p.into_inner()) = g;
}

/// Whether `core` is on now: Settings → Cores → Multiplayer
/// ([`super::cores::is_on`]), unless a test put a stand-in in front of it.
pub fn core_is_on(core: &str) -> bool {
    match *gate().read().unwrap_or_else(|p| p.into_inner()) {
        Some(g) => g(core),
        None => super::cores::is_on(core),
    }
}

/// Live sockets, for [`close_core`] and [`open_sockets`].
struct Handle {
    core: &'static str,
    key: DocKey,
    ctl: mpsc::Sender<Ctl>,
    /// Who and which connection: for `prev` (a reconnect retiring its old
    /// connection), matched on pid + doc + client id.
    pid: String,
    client: u64,
    session: Arc<Session>,
    conn: ConnId,
    /// Set when a reconnect retired this connection: its worker applies no
    /// client frame after it.
    dead: Arc<AtomicBool>,
}

/// What a socket that found its core off heard from the flip.
enum Flip {
    /// [`close_core`]'s details.
    Off { by: Option<String>, on_disk: Option<bool> },
    /// A reconnect or a rename ended it meanwhile: this frame.
    Other(Closed),
    /// Nothing within [`FLIP_WAIT`].
    None,
}

enum Ctl {
    /// A reconnect from the same tab retired this connection (`prev`).
    Replaced,
    /// The core was switched off: `closed`, then leave. `on_disk`: the
    /// kit's host close already wrote the doc back (`Some(true)`) or failed
    /// to (`Some(false)`); `None`: the session wasn't closed by it, so the
    /// worker writes back itself.
    CoreOff { by: Option<String>, on_disk: Option<bool> },
    /// The doc was renamed (`k2 doc mv`): `closed {reason: "moved", path}`.
    Moved { path: String },
    /// The text doc became a tier-0 doc (the kit's demote, [`doc_demoted`]):
    /// the core-off close, so the view reopens the file the usual way.
    /// `written`: the demotion wrote the live state to the file.
    Demoted { written: bool },
}

fn sockets() -> &'static Mutex<HashMap<u64, Handle>> {
    static S: OnceLock<Mutex<HashMap<u64, Handle>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_socket_id() -> u64 {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

/// Turning a core off, the Live half (called by the settings flip,
/// `cores::sync_with`, right after the kit's host close of the core's
/// sessions): end every Live socket on its files with `closed
/// {reason:"closed", code:"core_off", core, version, written}` and close
/// code 4010. `by` (a display name) goes in the frame's message. `on_disk`
/// is the host close's outcome per doc (written back, or the write-back
/// failed); a socket whose doc isn't in it writes back itself. Returns how
/// many sockets were told. A socket also notices a closed session or a
/// switched-off core on its own (within a second), and then waits briefly
/// for this call's details.
pub fn close_core(core: &str, by: Option<&str>, on_disk: &HashMap<DocKey, bool>) -> usize {
    let s = sockets().lock().unwrap_or_else(|p| p.into_inner());
    let mut n = 0;
    for h in s.values().filter(|h| h.core == core) {
        let ctl = Ctl::CoreOff { by: by.map(str::to_string), on_disk: on_disk.get(&h.key).copied() };
        if h.ctl.send(ctl).is_ok() {
            n += 1;
        }
    }
    n
}

/// A reconnect's `prev=<old client id>` (the reconnect-race rule, agreed
/// with K2 Cores for WIRE.md): when that connection is this person's, on this
/// doc and root, on this daemon, it is marked dead (its worker applies no
/// client frame from here on), it leaves the kit's session (an edit it was
/// applying has committed by then: `leave` takes the session lock), and its
/// socket closes. All before the new connection joins, so nothing from the
/// old socket applies after the new one's `since`. Anything else (unknown,
/// another person's, another doc's) is ignored silently: the caller learns
/// nothing about other connections. Returns whether one was retired.
fn retire_prev(prev: u64, pid: &str, key: &DocKey) -> bool {
    let found = {
        let s = sockets().lock().unwrap_or_else(|p| p.into_inner());
        s.values()
            .find(|h| h.client == prev && h.pid == pid && h.key == *key)
            .map(|h| (h.dead.clone(), h.session.clone(), h.conn, h.ctl.clone()))
    };
    let Some((dead, session, conn, ctl)) = found else { return false };
    dead.store(true, Ordering::SeqCst);
    let _ = session.leave(conn);
    let _ = ctl.send(Ctl::Replaced);
    true
}

/// **Called by `Runtime::rekey`:** the doc at `old` now lives at `new`.
/// Every Live socket on it ends with `closed {reason: "moved", path}` (the
/// view reopens the file at its new path). Returns how many were told.
pub fn doc_moved(old: &DocKey, new: &DocKey) -> usize {
    let s = sockets().lock().unwrap_or_else(|p| p.into_inner());
    s.values().filter(|h| h.key == *old).filter(|h| h.ctl.send(Ctl::Moved { path: new.path.clone() }).is_ok()).count()
}

/// **Called by `Runtime::open_blob` after the kit's demote:** the text doc
/// at `key` is a tier-0 doc now (its name routes to tier 0), under the same
/// key. Every Live socket on one of the text session's `connections` ends
/// with the close a view already reopens on: `closed {reason: "closed",
/// code: "core_off", core, version, written}` and close code 4010, whose
/// message says why. The view then opens the file the usual way, at tier 0.
/// A socket that saw the session close first (the roster's `Closed`) waits
/// for this, as it waits for a core flip. Returns how many were told.
pub fn doc_demoted(key: &DocKey, connections: &[(ConnId, String)], written: bool) -> usize {
    let s = sockets().lock().unwrap_or_else(|p| p.into_inner());
    s.values()
        .filter(|h| h.key == *key && connections.iter().any(|(c, _)| *c == h.conn))
        .filter(|h| h.ctl.send(Ctl::Demoted { written }).is_ok())
        .count()
}

/// The docs with a Live socket open (the Settings row's "open now" line):
/// `(core, key)` once per socket.
pub fn open_sockets() -> Vec<(&'static str, DocKey)> {
    sockets().lock().unwrap_or_else(|p| p.into_inner()).values().map(|h| (h.core, h.key.clone())).collect()
}

// ── Who may open one ─────────────────────────────────────────────────

/// The person behind an upgrade, or the 403 that refuses it before the
/// upgrade. Owner token and Connect logins only.
pub fn person_for(caller: crate::sidecar_routes::Caller, app_pass: bool) -> Result<DocCaller, CliResponse> {
    use crate::sidecar_routes::Caller;
    if app_pass {
        return Err(refusal("403 Forbidden", "app_docs_not_available", "the Live view isn't on the App surface yet"));
    }
    match caller {
        Caller::Owner => Ok(DocCaller::Person { key: k2_core::notify_prefs::OWNER.to_string(), via: VIA_TAB }),
        Caller::Login { username } => {
            let key = k2_core::notify_prefs::People::load()
                .map(|p| p.person_key(&username))
                .unwrap_or_else(|_| username.trim().to_ascii_lowercase());
            Ok(DocCaller::Person { key, via: VIA_TAB })
        }
        Caller::Agent(_) | Caller::Shell { .. } => Err(refusal(
            "403 Forbidden",
            "agents_use_k2_doc",
            "the Live socket is for a person's tab; agents edit live docs with k2 doc",
        )),
        Caller::WrongCredential => Err(refusal("403 Forbidden", "wrong_credential", "this credential can't open the Live view")),
        Caller::Invalid => Err(refusal("403 Forbidden", "forbidden", "invalid or missing token")),
    }
}

// ── Opening ──────────────────────────────────────────────────────────

/// A joined, subscribed connection and its first frame.
struct Opened {
    session: Arc<Session>,
    conn: ConnId,
    pid: String,
    key: DocKey,
    core: &'static str,
    client: u64,
    version: Seq,
    changes: Receiver<ChangeEvent>,
    roster: Receiver<RosterEvent>,
    welcome: Value,
    /// What the welcome listed in `features` (`keyed_watch` for a deck the
    /// kit can watch by key; none for a text doc).
    features: Vec<&'static str>,
    /// A live keyed doc: a deck, a timeline or a scene (watched; the preview
    /// service follows a deck's and a scene's changes).
    keyed: bool,
    rel: String,
    /// The doc's host events (and every turn end).
    events: super::host_events::Subscription,
}

fn target_reason(r: &target::Refusal) -> CloseReason {
    if r.status.starts_with("404") {
        CloseReason::NotFound
    } else {
        CloseReason::Declined
    }
}

/// What a refused open says, from the kit.
fn open_error(e: &SessionError) -> Closed {
    match e {
        SessionError::Declined(d) => Closed {
            code: Some(d.code.to_string()),
            message: Some(d.message.clone()),
            ..Closed::kit(CloseReason::Declined, e)
        },
        SessionError::PackMismatch { .. } => Closed::k2(
            CloseReason::Declined,
            "stored_as_any_file",
            "it was opened as an any-file doc earlier, and stays one",
        ),
        SessionError::Doc(DocError::Quarantined) => Closed {
            code: Some("doc_quarantined".into()),
            message: Some(super::routes::QUARANTINED_HINT.to_string()),
            ..Closed::kit(CloseReason::Declined, e)
        },
        // Kit v0.10.2 (MPC-E520): another engine or stored format wrote this
        // doc, so it is not loaded. The kit's line names which.
        SessionError::EngineMismatch { .. } => Closed {
            code: Some("engine_mismatch".into()),
            message: Some(e.to_string()),
            ..Closed::kit(CloseReason::Declined, e)
        },
        // Kit v0.20.0 (MPC-E169): a newer K2 opened this doc on the native
        // engine, which this build doesn't have. Only this doc is refused;
        // the kit's line, then K2's next step.
        SessionError::NativeEngineMissing { .. } => Closed {
            code: Some("native_engine_missing".into()),
            message: Some(super::routes::native_engine_missing_hint(e)),
            ..Closed::kit(CloseReason::Declined, e)
        },
        SessionError::NotFound(_) =>Closed::kit(CloseReason::NotFound, e),
        SessionError::Forbidden { .. } => Closed::kit(CloseReason::Auth, e),
        SessionError::Released => restart(),
        SessionError::Deleted => Closed::kit(CloseReason::Deleted, e),
        // K2 closed the session (kit v0.9.7.2's host close: a core switched
        // off) between the open and the join, twice.
        SessionError::Closed { .. } => Closed::kit(CloseReason::Closed, e),
        _ => Closed::kit(CloseReason::Declined, e),
    }
}

/// A refused open of a file whose core is off: one `closed` frame, no retry.
fn core_off_at_open(core: &str) -> Closed {
    Closed {
        core: Some(core.to_string()),
        ..Closed::k2(CloseReason::Closed, "core_off", format!("the {core} core is off on this server: open the file the usual way"))
    }
}

/// [`super::routes::open`]'s refusal as the socket's: the core gate's
/// `core_off`, or the kit's error.
fn route_open_error(e: super::routes::OpenError, core: &str) -> Closed {
    match e {
        super::routes::OpenError::CoreOff(_) => core_off_at_open(core),
        super::routes::OpenError::Session(e) => open_error(&e),
    }
}

/// The file a Live socket names, resolved and jailed, or why it can't be a
/// live view: the target's refusal (the deny list: `protected` for the vault,
/// git's folder and the workspace's private `.k2` folder; `secret`,
/// `generated`, `workspace_not_found`…), a bad root, a drawing, or not text.
/// Each one carries its own code and the daemon's words, so a tab never says
/// "too large, or not text" for a rule. An absolute path in another
/// workspace resolves to that workspace (the caller's rights decide next).
fn live_target(caller: &DocCaller, params: &HashMap<String, String>) -> Result<Target, Closed> {
    let root_param = params.get("root").map(|r| r.trim()).filter(|r| !r.is_empty());
    let path = params.get("doc").or_else(|| params.get("path")).cloned().unwrap_or_default();
    let t: Target = target::resolve(params.get("workspace").map(String::as_str), &path, caller.default_workspace())
        .map_err(|r| Closed::k2(target_reason(&r), r.code, r.message))?;
    // The Live view edits text; a live keyed doc (a deck, a timeline or a
    // scene, its core on) is watched ([`super::live_deck`]); any other file
    // is refused.
    let keyed = t.tier.is_keyed();
    if keyed && !super::live_deck::root_ok_for(t.tier, root_param) {
        return Err(Closed::k2(
            CloseReason::Declined,
            "bad_request",
            format!("a {}'s root is `{}` (or `{ROOT}`)", t.tier.family(), super::live_deck::root_of(t.tier)),
        ));
    }
    if !keyed && root_param.is_some_and(|r| r != ROOT) {
        return Err(Closed::k2(CloseReason::Declined, "bad_request", format!("root must be `{ROOT}`")));
    }
    // A drawing (the CAD core on) has no live view and K2 writes none: the
    // write routes' reason, so a client reads one answer (`cad_read_only`).
    if let Some((code, why)) = super::cad_read_only::socket_decline(&t.rel) {
        return Err(Closed::k2(CloseReason::Declined, code, why.to_string()));
    }
    if !keyed && !matches!(t.tier, Tier::Markdown | Tier::Text(_)) {
        return Err(Closed::k2(
            CloseReason::Declined,
            "not_a_text_doc",
            format!("{} isn't a text file; the Live view edits Markdown and other text", t.rel),
        ));
    }
    Ok(t)
}

/// Resolve, gate, open, join, subscribe, read. Blocking.
fn open_live(caller: &DocCaller, params: &HashMap<String, String>) -> Result<Opened, Closed> {
    let rt = super::runtime().map_err(|u| match &u {
        super::Unavailable::Failed(_) => Closed::k2(CloseReason::Declined, "docs_unavailable", u.message()),
        _ => restart(),
    })?;
    let resume = match params.get("resume").map(|r| r.trim()).filter(|r| !r.is_empty()) {
        Some(r) => Some(
            r.parse::<u64>()
                .map_err(|_| Closed::k2(CloseReason::Declined, "bad_request", "resume must be a whole number"))?,
        ),
        None => None,
    };
    // A malformed `prev` is ignored like an unknown one.
    let prev = params.get("prev").and_then(|v| v.trim().parse::<u64>().ok());
    let t = live_target(caller, params)?;
    let core = super::cores::core_of(t.tier);
    let keyed = t.tier.is_keyed();
    if !core_is_on(core) {
        return Err(core_off_at_open(core));
    }
    let key = t.key();
    let pid = caller.pid();
    let rights = {
        let store = rt.store();
        host::rights_for(&pid, &key, store.as_deref())
    };
    if !rights.read {
        return Err(Closed::k2(CloseReason::Auth, "doc_forbidden", "you can't read this doc"));
    }
    let participant = caller.participant(None);
    if let Some(prev) = prev {
        if retire_prev(prev, &pid, &key) {
            log_debug!("[docs/live] a reconnect retired its previous connection on {}", host::key_label(&key));
        }
    }
    let mut session = super::routes::open(&rt, &t).map_err(|e| route_open_error(e, core))?;
    // The connection's label on the roster (kit v0.10.1, `Device::label`:
    // "another tab", "Chrome on Mac"), shown beside the caret of a person's
    // other tabs. None for now. TODO(K2): where it comes from is K2's call:
    // a socket param the app sends (`?device=`; the app knows its own window
    // and machine), or the socket's User-Agent mapped to a short name. Never
    // doc text; the kit trims it to 64 characters.
    let join_opts = JoinOptions::new();
    let joined = match session.join_with(participant.clone(), Stay::Held, &join_opts) {
        // Unloaded, or closed by K2 (a core switched off), between the open
        // and the join: open once more (the gate then says core_off).
        Err(SessionError::Unloaded | SessionError::Closed { .. }) => {
            session = super::routes::open(&rt, &t).map_err(|e| route_open_error(e, core))?;
            session.join_with(participant, Stay::Held, &join_opts)
        }
        r => r,
    }
    .map_err(|e| open_error(&e))?;
    let conn = joined.conn;
    // From here on a failure leaves before it answers.
    let fail = |s: &Session, e: SessionError| {
        let _ = s.leave(conn);
        open_error(&e)
    };
    // A text doc's last outside save, when recent, comes first (a tab that
    // joined after it still gets its `outside_save` and `before_version`).
    let events = super::host_events::subscribe_replaying(&key, resume, host::now_ms());
    let changes = session.subscribe(conn).map_err(|e| fail(&session, e))?;
    let roster = session.subscribe_roster(conn).map_err(|e| fail(&session, e))?;
    if keyed {
        let version = session.status().map_err(|e| fail(&session, e))?.version;
        let mut welcome = wire::welcome_frame(
            super::live_deck::root_of(t.tier),
            version,
            None,
            joined.client,
            &pid,
            wire::rights(DocRights::kit(rights)),
            core,
            t.tier.family(),
            &t.rel,
        );
        // The keyed watch (kit v0.16): listed only for a doc the kit can
        // watch by key, never a text doc or a table. Then the view address
        // and the time position (kit v0.18.0), for every person here.
        // A scene: the view address and presence (and claims on its objects
        // once people may change it; read-only for now), no time.
        let features = match t.tier {
            Tier::Scene => super::live_watch::scene_socket_features(&session, !super::blend_read_only::writes_off(&t.rel)),
            _ => super::live_watch::keyed_socket_features(&session),
        };
        if let Value::Obj(o) = &mut welcome {
            o.extend(super::live_deck::welcome_extras(&t, &session, conn));
            o.push(("access_owner".into(), super::live_deck::actor_record(&host::access_owner_pid())));
            if !features.is_empty() {
                o.push(("features".into(), super::live_watch::features_value(&features)));
            }
            // A `.blend` K2 can't write yet: its `rights` is `read` for
            // everyone, and the welcome says why, so the view shows it from
            // the start (`blend_read_only`).
            for (k, v) in super::blend_read_only::welcome_fields(&t.rel) {
                o.push((k.into(), Value::from(v)));
            }
        }
        return Ok(Opened {
            session,
            conn,
            pid,
            key,
            core,
            client: joined.client,
            version,
            changes,
            roster,
            welcome,
            features,
            keyed: true,
            rel: t.rel.clone(),
            events,
        });
    }
    let read = session.read(conn).map_err(|e| fail(&session, e))?;
    let text = read
        .text_by_root
        .iter()
        .find(|(r, _)| r == ROOT)
        .map(|(_, t)| t.clone())
        .ok_or_else(|| fail(&session, SessionError::BadEdit(format!("this doc has no `{ROOT}` text"))))?;
    // A resume the window still covers needs no text: the client asks
    // `since` next. Ahead of the doc, or past the window: the whole text.
    let with_text = match resume {
        None => true,
        Some(v) if v > read.version => true,
        Some(v) => session.events_since(conn, v).is_err(),
    };
    let mut welcome = wire::welcome_frame(
        ROOT,
        read.version,
        with_text.then_some(text.as_str()),
        joined.client,
        &pid,
        wire::rights(DocRights::kit(rights)),
        core,
        t.tier.family(),
        &t.rel,
    );
    // A text doc's socket takes the view address too (kit v0.18.0: Follow
    // by heading, `@k:` or a native address), never the keyed watch.
    let features = super::live_watch::TEXT_SOCKET_FEATURES.to_vec();
    if let Value::Obj(o) = &mut welcome {
        o.push(("features".into(), super::live_watch::features_value(&features)));
        // Who to ask for edit access (the read-only bar), as an actor record.
        o.push(("access_owner".into(), super::live_deck::actor_record(&host::access_owner_pid())));
    }
    Ok(Opened {
        session,
        conn,
        pid,
        key,
        core,
        client: joined.client,
        version: read.version,
        changes,
        roster,
        welcome,
        features,
        keyed: false,
        rel: t.rel.clone(),
        events,
    })
}

// ── The socket ───────────────────────────────────────────────────────

/// What the worker sends the socket task.
enum Out {
    Text(String),
    /// The last frame (none for `network`), then the close.
    Close(Closed),
}

/// What the socket task sends the worker.
enum Inbound {
    Frame(String),
    /// A text frame over the cap: refused in order (an `edit`'s `op` when
    /// readable: `refused`; anything else: `error`).
    TooBig { edit_op: Option<u64>, len: usize },
}

/// Serve one upgrade. The dispatcher resolved `caller` (a person) and ran
/// its gates.
pub async fn serve(stream: &mut TcpStream, params: HashMap<String, String>, caller: DocCaller, login: crate::socket_access::LoginCheck) {
    let mut cfg = WebSocketConfig::default();
    cfg.max_message_size = Some(wire::HARD_CAP);
    cfg.max_frame_size = Some(wire::HARD_CAP);
    let ws = match tokio_tungstenite::accept_async_with_config(&mut *stream, Some(cfg)).await {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[docs/live] handshake failed: {e}");
            return;
        }
    };
    let (mut write, mut read) = ws.split();

    let opened = {
        let caller = caller.clone();
        tokio::task::spawn_blocking(move || open_live(&caller, &params)).await
    };
    let opened = match opened {
        Ok(Ok(o)) => o,
        Ok(Err(closed)) => {
            finish(&mut write, &closed).await;
            return;
        }
        Err(e) => {
            log_debug!("[docs/live] open failed: {e}");
            finish(&mut write, &restart()).await;
            return;
        }
    };
    let welcome = opened.welcome.to_json();
    if welcome.len() > wire::FRAME_MAX {
        // The text doesn't fit one frame: no live editing here (E401).
        let _ = opened.session.leave(opened.conn);
        finish(&mut write, &wire::too_large(welcome.len())).await;
        return;
    }
    if write.send(Message::Text(welcome)).await.is_err() {
        let _ = opened.session.leave(opened.conn);
        return;
    }

    let id = next_socket_id();
    let (in_tx, in_rx) = mpsc::sync_channel::<Inbound>(INBOX);
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Out>(OUTBOX);
    let (ctl_tx, ctl_rx) = mpsc::channel::<Ctl>();
    let dead = Arc::new(AtomicBool::new(false));
    sockets()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(
            id,
            Handle {
                core: opened.core,
                key: opened.key.clone(),
                ctl: ctl_tx,
                pid: opened.pid.clone(),
                client: opened.client,
                session: opened.session.clone(),
                conn: opened.conn,
                dead: dead.clone(),
            },
        );
    let worker = Worker::new(opened, in_rx, ctl_rx, out_tx, dead, login);
    let spawned = std::thread::Builder::new().name(format!("k2-docs-live-{id}")).spawn(move || worker.run());
    if let Err(e) = spawned {
        log_debug!("[docs/live] WARN: worker thread: {e}");
        sockets().lock().unwrap_or_else(|p| p.into_inner()).remove(&id);
        return;
    }

    let every = ping_interval();
    let dead_after = every * 2 + every / 2;
    let mut keepalive = tokio::time::interval(every);
    keepalive.tick().await; // the immediate first tick
    let mut heard = Instant::now();
    let mut linger = false;
    // Did this side send a Close the client is still to answer?
    let mut closing = false;
    loop {
        tokio::select! {
            biased;
            out = out_rx.recv() => match out {
                Some(Out::Text(t)) => {
                    if write.send(Message::Text(t)).await.is_err() {
                        break;
                    }
                }
                Some(Out::Close(c)) => {
                    finish(&mut write, &c).await;
                    closing = true;
                    break;
                }
                // The worker ended without a frame (the client left).
                None => break,
            },
            _ = keepalive.tick() => {
                if heard.elapsed() > dead_after {
                    log_debug!("[docs/live] socket {id}: no pong for {} ms; closing", dead_after.as_millis());
                    let _ = write.send(close_msg(code_normal(), "network")).await;
                    break;
                }
                if write.send(Message::Ping(Vec::new())).await.is_err() {
                    break;
                }
            }
            incoming = read.next() => {
                heard = Instant::now();
                match incoming {
                    Some(Ok(Message::Text(t))) => {
                        let msg = if t.len() > wire::FRAME_MAX {
                            let (ty, op) = wire::type_and_op(&t);
                            Inbound::TooBig { edit_op: op.filter(|_| ty.as_deref() == Some("edit")), len: t.len() }
                        } else {
                            Inbound::Frame(t)
                        };
                        if in_tx.try_send(msg).is_err() {
                            log_debug!("[docs/live] socket {id}: client frames piling up; cutting off");
                            let c = Closed::k2(CloseReason::CutOff, "too_many_frames", "frames arrived faster than the doc could take them");
                            finish(&mut write, &c).await;
                            closing = true;
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(p))) => {
                        if write.send(Message::Pong(p)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Binary(_))) => {
                        let e = wire::error_frame(&wire::unreadable("frames are JSON text, not binary"));
                        if write.send(Message::Text(e.to_json())).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Frame(_))) => {}
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => {
                        // Over the WebSocket layer's own limit: say so, then close.
                        if matches!(e, tokio_tungstenite::tungstenite::Error::Capacity(_)) {
                            let c = Closed {
                                error: Some(wire::unreadable(&format!("a frame over the socket's {}-byte limit", wire::HARD_CAP))),
                                ws_code: wire::code::TOO_BIG,
                                ..Closed::new(CloseReason::CutOff)
                            };
                            finish(&mut write, &c).await;
                            linger = true;
                        }
                        break;
                    }
                }
            }
        }
    }
    // Dropping the inbox tells the worker the socket is gone: it leaves.
    drop(in_tx);
    sockets().lock().unwrap_or_else(|p| p.into_inner()).remove(&id);
    if closing {
        await_close_reply(&mut read).await;
    }
    if linger {
        // The rest of the oversized frame is still arriving: read and drop
        // it (a few seconds at most), so the close isn't lost to a reset
        // and the client reads the `closed` frame.
        drop(write);
        drop(read);
        let mut buf = vec![0u8; 64 << 10];
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            while let Ok(n) = stream.read(&mut buf).await {
                if n == 0 {
                    break;
                }
            }
        })
        .await;
    }
}

fn code_normal() -> u16 {
    wire::code::NORMAL
}

/// The daemon is handing off or stopping (or live docs are still
/// starting): no `closed` frame, close code 1012. The transport reports a
/// dropped socket as `network`, and the component reconnects to whichever
/// daemon serves next.
fn restart() -> Closed {
    Closed { ws_code: wire::code::RESTART, ..Closed::new(CloseReason::Network) }
}

/// How long a socket that sent its Close waits for the client's.
const CLOSE_REPLY_WAIT: Duration = Duration::from_secs(2);

/// The closing handshake's second half (RFC 6455 §7.1.1): after its Close the
/// server reads until the client's Close, then drops the connection. Dropping
/// it with a client frame still unread (a Pong to the last keepalive, an edit
/// in flight) makes the kernel answer with a reset instead of a FIN, and a
/// reset can take the client's unread bytes with it: the client gets the
/// `closed` frame but loses the Close behind it, and with it the close code
/// (4010, 4003...). Frames read here are dropped; a client that never
/// answers is let go after [`CLOSE_REPLY_WAIT`].
async fn await_close_reply<S>(read: &mut S)
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let _ = tokio::time::timeout(CLOSE_REPLY_WAIT, async {
        while let Some(Ok(m)) = read.next().await {
            if matches!(m, Message::Close(_)) {
                break;
            }
        }
    })
    .await;
}

/// The last frame (none for `network`: "never in a frame") and the close.
async fn finish<S>(write: &mut S, c: &Closed)
where
    S: futures_util::Sink<Message> + Unpin,
{
    if c.reason != CloseReason::Network {
        let _ = write.send(Message::Text(c.value().to_json())).await;
    }
    let _ = write.send(close_msg(c.ws_code, c.reason.as_str())).await;
}

/// A client's caret on `root` at `version` (bytes of that version's text),
/// set in the kit at the doc's current version (WIRE.md "Carets"). The kit's
/// `set_cursor` takes no version, so a caret from an older version is mapped
/// first ([`map_caret`]). Fire-and-forget: a dropped or refused caret is only
/// logged. Returns the caret as set.
fn place_caret(session: &Session, conn: ConnId, root: &str, version: Seq, at_seen: Seq, anchor: usize, head: usize) -> Option<(usize, usize)> {
    let now = match session.status() {
        Ok(s) => s.version,
        Err(e) => {
            log_debug!("[docs/live] caret ignored: {}", e.report().plain());
            return None;
        }
    };
    let (a, h) = map_caret(session, conn, root, version, at_seen, now, anchor, head)?;
    match session.set_cursor(conn, root, At::Byte(a), At::Byte(h)) {
        Ok(c) => Some((c.anchor, c.head)),
        Err(e) => {
            log_debug!("[docs/live] caret ignored: {}", e.report().plain());
            None
        }
    }
}

/// Where a caret at `version` is at `now` (the doc's version): unchanged when
/// they're equal; behind, `anchor` and `head` are carried through every change
/// since on `root`, in order, with the bias the kit carries carets with
/// (`Bias::After`). `None` (dropped) for another root, a version past
/// `at_seen` (the client can't have seen it), or one `events_since` refuses
/// (past the live window).
#[allow(clippy::too_many_arguments)]
fn map_caret(session: &Session, conn: ConnId, root: &str, version: Seq, at_seen: Seq, now: Seq, anchor: usize, head: usize) -> Option<(usize, usize)> {
    if root != ROOT {
        return None;
    }
    if version > at_seen || version > now {
        log_debug!("[docs/live] caret at {version} dropped: ahead of what this socket told the client ({at_seen})");
        return None;
    }
    if version == now {
        return Some((anchor, head));
    }
    let events = match session.events_since(conn, version) {
        Ok(evs) => evs,
        Err(e) => {
            log_debug!("[docs/live] caret at {version} dropped: {}", e.plain());
            return None;
        }
    };
    let (mut a, mut h) = (anchor, head);
    for e in events.iter().filter(|e| e.root == root) {
        a = transform(a, &e.hunks, Bias::After);
        h = transform(h, &e.hunks, Bias::After);
    }
    Some((a, h))
}

fn close_msg(code: u16, reason: &str) -> Message {
    Message::Close(Some(CloseFrame { code: CloseCode::from(code), reason: reason.to_string().into() }))
}

// ── The worker ───────────────────────────────────────────────────────

/// How a worker step ended the socket.
enum Stop {
    /// The socket is gone (the client left, or the socket task ended).
    Gone,
    /// Send this, then close.
    Close(Closed),
}

struct Worker {
    session: Arc<Session>,
    conn: ConnId,
    pid: String,
    key: DocKey,
    core: &'static str,
    changes: Receiver<ChangeEvent>,
    roster: Receiver<RosterEvent>,
    inbox: Receiver<Inbound>,
    ctl: Receiver<Ctl>,
    out: tokio::sync::mpsc::Sender<Out>,
    /// Retired by a reconnect (`prev`): no client frame applies any more.
    dead: Arc<AtomicBool>,
    /// One change taken off the kit's queue but not yet sent (a peek).
    peeked: Option<ChangeEvent>,
    /// The kit's change queue ended (the connection left the session).
    changes_gone: bool,
    /// The roster's last event said K2 closed the session (a core off).
    host_closed: bool,
    /// Versions of this connection's own edits not yet passed: their events
    /// are never forwarded (G3).
    own: HashSet<Seq>,
    /// The doc's version as this socket last told the client.
    seen: Seq,
    /// The person's credential: when it ends, so does this socket.
    login: crate::socket_access::LoginCheck,
    /// A live keyed doc (a deck, a timeline, a scene): watched, never edited
    /// here ([`super::live_deck`]).
    keyed: bool,
    rel: String,
    /// Host events for this doc, and every turn end.
    events: super::host_events::Subscription,
    /// Turns this doc's changes carried (a `turn_end` is sent only for those).
    turns: super::live_deck::Turns,
    /// The keyed watch, once the client sends `watch` ([`super::live_watch`]).
    watch: super::live_watch::SocketWatch,
    /// What the welcome listed (`view_address` and `time_position` are
    /// taken only when listed).
    features: Vec<&'static str>,
}

/// However the worker ends (a spawn that failed included), its connection
/// leaves the doc (`leave` on a gone connection is a no-op).
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.session.leave(self.conn);
    }
}

impl Worker {
    fn new(
        o: Opened,
        inbox: Receiver<Inbound>,
        ctl: Receiver<Ctl>,
        out: tokio::sync::mpsc::Sender<Out>,
        dead: Arc<AtomicBool>,
        login: crate::socket_access::LoginCheck,
    ) -> Self {
        Worker {
            session: o.session,
            conn: o.conn,
            pid: o.pid,
            key: o.key,
            core: o.core,
            changes: o.changes,
            roster: o.roster,
            inbox,
            ctl,
            out,
            dead,
            peeked: None,
            changes_gone: false,
            host_closed: false,
            own: HashSet::new(),
            seen: o.version,
            login,
            keyed: o.keyed,
            rel: o.rel,
            events: o.events,
            turns: super::live_deck::Turns::default(),
            watch: super::live_watch::SocketWatch::new(&o.features),
            features: o.features,
        }
    }

    fn run(mut self) {
        let stop = self.serve();
        match stop {
            Stop::Gone => {
                let _ = self.session.leave(self.conn);
            }
            Stop::Close(c) => {
                let _ = self.session.leave(self.conn);
                let _ = self.out.blocking_send(Out::Close(c));
            }
        }
    }

    fn serve(&mut self) -> Stop {
        let mut next_gate = Instant::now() + GATE_EVERY;
        let mut next_login = Instant::now() + crate::socket_access::TICK;
        loop {
            // Before anything more goes out or comes in.
            if !self.login.frame_ok() {
                return Stop::Close(self.signed_out());
            }
            if Instant::now() >= next_login {
                next_login = Instant::now() + crate::socket_access::TICK;
                if !self.login.valid_now() {
                    return Stop::Close(self.signed_out());
                }
            }
            if let Err(s) = self.pump() {
                return s;
            }
            if self.dead.load(Ordering::SeqCst) {
                return Stop::Close(Closed::new(CloseReason::Left));
            }
            match self.ctl.try_recv() {
                Ok(Ctl::Replaced) => return Stop::Close(Closed::new(CloseReason::Left)),
                Ok(Ctl::CoreOff { by, on_disk }) => return Stop::Close(self.core_off(by, on_disk)),
                Ok(Ctl::Demoted { written }) => return Stop::Close(self.demoted(written)),
                Ok(Ctl::Moved { path }) => {
                    return Stop::Close(Closed {
                        path: Some(path.clone()),
                        ..Closed::k2(CloseReason::Moved, "moved", format!("the file was renamed to {path}; open it there"))
                    })
                }
                Err(_) => {}
            }
            if Instant::now() >= next_gate {
                next_gate = Instant::now() + GATE_EVERY;
                if !core_is_on(self.core) {
                    // The switch went off: the flip's close and its details
                    // (who, written) follow at once. Without them, and with
                    // the switch on again (a blip: nothing closed this doc,
                    // and the session is still live), the socket stays;
                    // still off, it writes back itself and ends.
                    match self.wait_for_flip() {
                        Flip::Off { by, on_disk } => return Stop::Close(self.core_off(by, on_disk)),
                        Flip::Other(c) => return Stop::Close(c),
                        Flip::None if core_is_on(self.core) && self.session.touch(self.conn).is_ok() => {
                            log_debug!("[docs/live] the {} core was off for a moment and nothing closed {}; the socket stays", self.core, host::key_label(&self.key));
                        }
                        Flip::None => return Stop::Close(self.core_off(None, None)),
                    }
                }
            }
            match self.inbox.recv_timeout(POLL) {
                Ok(Inbound::Frame(_)) | Ok(Inbound::TooBig { .. }) if self.dead.load(Ordering::SeqCst) => {
                    // Retired by a reconnect: nothing from this socket applies.
                    return Stop::Close(Closed::new(CloseReason::Left));
                }
                Ok(Inbound::Frame(t)) => {
                    if let Err(s) = self.on_frame(&t) {
                        return s;
                    }
                }
                Ok(Inbound::TooBig { edit_op, len }) => {
                    let e = wire::over_cap(len);
                    let reply = match edit_op {
                        Some(op) => wire::refused_frame(op, &e),
                        None => wire::error_frame(&e),
                    };
                    if let Err(s) = self.send(reply) {
                        return s;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Stop::Gone,
            }
        }
    }

    fn send(&self, v: Value) -> Result<(), Stop> {
        self.send_text(v.to_json())
    }

    fn send_text(&self, t: String) -> Result<(), Stop> {
        self.out.blocking_send(Out::Text(t)).map_err(|_| Stop::Gone)
    }

    /// The next queued change, if any (non-blocking).
    fn peek(&mut self) -> Option<&ChangeEvent> {
        if self.peeked.is_none() && !self.changes_gone {
            match self.changes.try_recv() {
                Ok(e) => self.peeked = Some(e),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.changes_gone = true,
            }
        }
        self.peeked.as_ref()
    }

    /// Wait up to `d` for the next change.
    fn peek_wait(&mut self, d: Duration) -> Option<&ChangeEvent> {
        if self.peeked.is_none() && !self.changes_gone {
            match self.changes.recv_timeout(d) {
                Ok(e) => self.peeked = Some(e),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => self.changes_gone = true,
            }
        }
        self.peeked.as_ref()
    }

    /// Send one change on, unless it is this connection's own edit.
    fn forward(&mut self, e: ChangeEvent) -> Result<(), Stop> {
        self.seen = self.seen.max(e.version);
        if let Some(t) = &e.turn {
            self.turns.saw(t.as_str());
        }
        if self.keyed {
            super::live_deck::changed(&self.key, &self.rel, &self.session, self.conn);
        }
        if self.own.contains(&e.version) {
            return Ok(());
        }
        self.own.retain(|v| *v >= e.version);
        if self.watch.open() {
            // The keyed watch carries this change as `ops_change`.
            return Ok(());
        }
        let t = wire::change_frame(&e).to_json();
        if t.len() > wire::FRAME_MAX {
            // Withheld: the client sees the version gap and asks `since`.
            log_debug!("[docs/live] change {} ({} bytes) is over the frame cap; withheld", e.version, t.len());
            return Ok(());
        }
        self.send_text(t)
    }

    /// Forward every queued change with a version up to `upto` (all when
    /// `None`), then every queued roster event. Ends the socket when the kit
    /// dropped the connection.
    fn pump_upto(&mut self, upto: Option<Seq>) -> Result<(), Stop> {
        loop {
            let due = matches!(self.peek(), Some(e) if upto.is_none_or(|u| e.version <= u));
            if !due {
                break;
            }
            let e = self.peeked.take().expect("peeked");
            self.forward(e)?;
        }
        // The keyed watch's change frames, and a fresh snapshot when it
        // needs one (nothing before the client's `watch`).
        if !self.changes_gone {
            for t in self.watch.pump(&self.session) {
                self.send_text(t)?;
            }
        }
        loop {
            match self.roster.try_recv() {
                // K2 closed the session (the kit's host close: a core switched
                // off). It is the roster's last event; the socket ends with
                // its own `closed` frame instead (WIRE.md v2 "Closing").
                Ok(RosterEvent::Closed { .. }) => self.host_closed = true,
                // Everything else, `overwritten` included (kit v0.9.7.2: an
                // outside save may have removed this person's text; the kit
                // sends it to its author alone, and a snapshot carries the
                // ones still waiting).
                Ok(ev) => {
                    if let Some(v) = wire::roster_event_for(&ev, Value::from(self.pid.as_str())) {
                        self.send(wire::roster_frame(v))?;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.drain_changes()?;
                    return Err(Stop::Close(self.why_gone()));
                }
            }
        }
        // K2's host events: previews, turn ends, the comment relay.
        while let Ok(ev) = self.events.rx.try_recv() {
            if let Some(f) = super::live_deck::host_event_frame(&ev, &self.pid, &self.key, &self.turns) {
                self.send(f)?;
            }
        }
        if self.changes_gone && self.peeked.is_none() {
            return Err(Stop::Close(self.why_gone()));
        }
        Ok(())
    }

    fn pump(&mut self) -> Result<(), Stop> {
        self.pump_upto(None)
    }

    /// Every change still queued goes out (the kit dropped the connection:
    /// what it queued before is the client's).
    fn drain_changes(&mut self) -> Result<(), Stop> {
        while self.peek().is_some() {
            let e = self.peeked.take().expect("peeked");
            self.forward(e)?;
        }
        Ok(())
    }

    /// The person's token ended: the same words as the kit's own removal
    /// (WIRE.md "Closing"), and 4003 for a client that misses the frame.
    fn signed_out(&self) -> Closed {
        let store = super::runtime().ok().and_then(|rt| rt.store());
        let c = if host::rights_for(&self.pid, &self.key, store.as_deref()).read {
            Closed::k2(CloseReason::Kicked, "kicked", "your sign-in to this server ended: sign in again to keep editing")
        } else {
            Closed::k2(CloseReason::RightsLost, "rights_lost", "you can no longer read this doc")
        };
        Closed { ws_code: crate::presence::WS_CLOSE_SESSION_REVOKED, ..c }
    }

    /// The kit dropped this connection's queue: why.
    fn why_gone(&self) -> Closed {
        if self.dead.load(Ordering::SeqCst) {
            // A reconnect retired it (`prev`); the tab moved to the new socket.
            return Closed::new(CloseReason::Left);
        }
        if self.host_closed || !core_is_on(self.core) {
            return self.core_off_from_flip();
        }
        if let Err(super::Unavailable::Released) = super::runtime() {
            return restart();
        }
        match self.session.touch(self.conn) {
            // Still joined: the queue overflowed (1,024 events behind).
            Ok(()) => Closed::k2(CloseReason::CutOff, "cut_off", "this socket fell behind the doc; reconnect and resume"),
            Err(SessionError::Closed { .. }) => self.core_off_from_flip(),
            Err(e @ SessionError::Deleted) => Closed::kit(CloseReason::Deleted, &e),
            Err(SessionError::Released) => restart(),
            Err(e @ SessionError::Unloaded) => Closed::kit(CloseReason::CutOff, &e),
            Err(e @ SessionError::Forbidden { .. }) => Closed::kit(CloseReason::RightsLost, &e),
            Err(SessionError::NotJoined { .. }) => {
                let store = super::runtime().ok().and_then(|rt| rt.store());
                if host::rights_for(&self.pid, &self.key, store.as_deref()).read {
                    Closed::k2(CloseReason::Kicked, "kicked", "you were removed from this doc")
                } else {
                    Closed::k2(CloseReason::RightsLost, "rights_lost", "you can no longer read this doc")
                }
            }
            Err(e) => Closed::kit(CloseReason::Network, &e),
        }
    }

    /// The core is off, or K2 closed the session: the flip's details for
    /// this socket (who turned the core off, and whether the kit's host
    /// close wrote the doc back) come right after its close, so wait up to
    /// [`FLIP_WAIT`] for them; without them the socket writes back itself.
    fn core_off_from_flip(&self) -> Closed {
        match self.wait_for_flip() {
            Flip::Off { by, on_disk } => self.core_off(by, on_disk),
            Flip::Other(c) => c,
            Flip::None => self.core_off(None, None),
        }
    }

    /// Up to [`FLIP_WAIT`] for the flip's [`close_core`]. A reconnect or a
    /// rename meanwhile ends the socket its own way.
    fn wait_for_flip(&self) -> Flip {
        let deadline = Instant::now() + FLIP_WAIT;
        match self.ctl.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Ctl::CoreOff { by, on_disk }) => Flip::Off { by, on_disk },
            Ok(Ctl::Replaced) => Flip::Other(Closed::new(CloseReason::Left)),
            Ok(Ctl::Demoted { written }) => Flip::Other(self.demoted(written)),
            Ok(Ctl::Moved { path }) => Flip::Other(Closed {
                path: Some(path.clone()),
                ..Closed::k2(CloseReason::Moved, "moved", format!("the file was renamed to {path}; open it there"))
            }),
            Err(_) => Flip::None,
        }
    }

    /// The core was switched off: the frame, after the doc is on disk. The
    /// kit's host close wrote it back (`on_disk`); a session it didn't close
    /// is written back here.
    fn core_off(&self, by: Option<String>, on_disk: Option<bool>) -> Closed {
        let written = on_disk.unwrap_or_else(|| self.session.flush(self.conn).is_ok());
        let version = self.session.status().map(|s| s.version).unwrap_or(self.seen);
        let message = match by {
            Some(b) => format!("the {} core was turned off by {b}: this file now opens the usual way", self.core),
            None => format!("the {} core was turned off: this file now opens the usual way", self.core),
        };
        Closed {
            core: Some(self.core.to_string()),
            version: Some(version),
            written: Some(written),
            ws_code: wire::code::CORE_OFF,
            ..Closed::k2(CloseReason::Closed, "core_off", message)
        }
    }

    /// The text doc became a tier-0 doc ([`doc_demoted`]): the core-off
    /// close (the one a view reopens the file on), at the last version this
    /// socket saw (the text session is gone), and why in its message.
    fn demoted(&self, written: bool) -> Closed {
        Closed {
            core: Some(self.core.to_string()),
            version: Some(self.seen),
            written: Some(written),
            ws_code: wire::code::CORE_OFF,
            ..Closed::k2(
                CloseReason::Closed,
                "core_off",
                "K2 no longer edits this kind of file as live text: it is an any-file doc now, with every version kept. It reopens the usual way",
            )
        }
    }

    fn on_frame(&mut self, text: &str) -> Result<(), Stop> {
        let f = match wire::parse(text) {
            Ok(f) => f,
            Err(b) => return self.send(b.reply()),
        };
        if self.keyed {
            // A `.blend` K2 can't write: a person's edit, undo or redo is
            // refused with the write routes' reason (`blend_writer_disabled`),
            // before anything reaches the kit (`blend_read_only`).
            if let Some(reply) = super::blend_read_only::socket_refusal(&f, &self.rel) {
                return self.send(reply);
            }
            if let Some(reply) = super::live_deck::refuse(&f) {
                return self.send(reply);
            }
        }
        // Anything that writes or reads the doc asks the store first, never
        // only the last tick. Carets and view hints wait for the next poll.
        let touches_doc = matches!(
            f,
            ClientFrame::Edit { .. }
                | ClientFrame::Since { .. }
                | ClientFrame::Undo { .. }
                | ClientFrame::Redo { .. }
                | ClientFrame::Snapshot { .. }
                | ClientFrame::Watch
        );
        if touches_doc && !self.login.valid_now() {
            return Err(Stop::Close(self.signed_out()));
        }
        match f {
            ClientFrame::Edit { op, root, base, hunks, group } => self.edit(op, &root, base, hunks, group),
            ClientFrame::Since { op, version } => match self.session.events_since(self.conn, version) {
                Ok(evs) => {
                    let t = wire::events_frame(op, &evs).to_json();
                    if t.len() > wire::FRAME_MAX {
                        return self.send(wire::refused_frame(op, &wire::events_over_cap(t.len())));
                    }
                    self.send_text(t)
                }
                Err(r) => self.send(wire::refused_frame(op, &r)),
            },
            ClientFrame::Undo { op } => self.undo(op, true),
            ClientFrame::Redo { op } => self.undo(op, false),
            ClientFrame::Snapshot { op } => match self.session.read(self.conn) {
                Ok(r) => {
                    let text = r.text_by_root.iter().find(|(k, _)| k == ROOT).map(|(_, t)| t.as_str()).unwrap_or("");
                    let t = wire::snapshot_frame(op, text, r.version).to_json();
                    if t.len() > wire::FRAME_MAX {
                        return Err(Stop::Close(wire::too_large(t.len())));
                    }
                    self.send_text(t)
                }
                Err(e) => self.send(wire::refused_frame(op, &e.report())),
            },
            ClientFrame::Cursor { root, version, anchor, head } => {
                // G6: the kit places a caret in the current text. The client
                // knows at most what this socket had told it before this
                // frame (`at_seen`); the pump may tell it more, after.
                let at_seen = self.seen;
                self.pump()?;
                place_caret(&self.session, self.conn, &root, version, at_seen, anchor, head);
                Ok(())
            }
            ClientFrame::Visible(v) => {
                let _ = self.session.set_visible(self.conn, v);
                Ok(())
            }
            // The keyed watch: a snapshot, then its change frames from the
            // pump. Not listed in this socket's welcome: `MPC-E113`.
            ClientFrame::Watch => match self.watch.on_watch(&self.session, self.conn) {
                Ok(frames) => {
                    for t in frames {
                        self.send_text(t)?;
                    }
                    Ok(())
                }
                Err(n) => self.send(wire::error_frame(&wire::report("MPC-E113", n.0))),
            },
            ClientFrame::Report { .. } => match self.watch.on_report() {
                Ok(()) => Ok(()),
                Err(n) => self.send(wire::error_frame(&wire::report("MPC-E113", n.0))),
            },
            ClientFrame::Hello => Ok(()),
            // Kit v0.18.0 presence (WIRE.md "Presence frames"): the view
            // this person looks at and their playhead, on the roster as
            // `view_address` / `time_position`. No reply unless refused
            // (`presence_refused {req}`); not listed here: `MPC-E113`.
            ClientFrame::ViewAddress { req, address } => {
                self.presence(req, super::live_watch::VIEW_ADDRESS_FEATURE, |s, c| {
                    s.set_view_address(c, address.as_deref()).map(drop)
                })
            }
            // Presence by element (WIRE.md "Presence frames", feature
            // `presence`): a selection by key under a root, or by address;
            // a pointer; a viewport. Read right only.
            ClientFrame::Select { req, selection } => self.presence(req, super::live_watch::PRESENCE_FEATURE, |s, c| {
                let sel = match selection {
                    None => None,
                    Some((Some(root), keys)) => {
                        let keys = keys
                            .iter()
                            .map(|k| k2_mpc_base::elem::Key::new(k.as_str()).map_err(|e| SessionError::BadPosition(format!("{k:?} isn't a key: {e}"))))
                            .collect::<Result<Vec<_>, _>>()?;
                        Some(k2_mpc_base::session::ElemSelection::new(root, keys))
                    }
                    Some((None, addresses)) => {
                        let mut root: Option<String> = None;
                        let mut keys = Vec::new();
                        for a in &addresses {
                            let (r, k) = s.resolve_address(c, a)?;
                            if root.as_deref().is_some_and(|x| x != r) {
                                return Err(SessionError::BadPosition("a selection is in one root: these addresses are in two".into()));
                            }
                            root = Some(r);
                            keys.push(k);
                        }
                        let root = root.ok_or_else(|| SessionError::BadPosition("a selection names at least one element (null clears it)".into()))?;
                        Some(k2_mpc_base::session::ElemSelection::new(root, keys))
                    }
                };
                s.set_selection(c, sel).map(drop)
            }),
            ClientFrame::Pointer { req, point } => self.presence(req, super::live_watch::PRESENCE_FEATURE, |s, c| {
                let p = point.map(|(x, y)| k2_mpc_base::session::Point::new(x, y)).transpose()?;
                s.set_pointer(c, p)
            }),
            ClientFrame::Viewport { req, rect } => self.presence(req, super::live_watch::PRESENCE_FEATURE, |s, c| {
                let r = rect.map(|(x, y, w, h)| k2_mpc_base::session::Rect::new(x, y, w, h)).transpose()?;
                s.set_viewport(c, r)
            }),
            ClientFrame::TimePosition { req, time } => {
                self.presence(req, super::live_watch::TIME_POSITION_FEATURE, |s, c| {
                    let time = time.as_ref().map(wire::TimeSet::position).transpose()?;
                    s.set_time_position(c, time).map(drop)
                })
            }
            // Claims on elements (WIRE.md "Claim frames"; a scene's welcome
            // lists `claims`): by `root` and keys, or by address with no
            // root. `claimed {req, claim}` / `released {req, claims}`; the
            // kit's refusal is `presence_refused {req, error}`. Everyone sees
            // them on the roster (`claim_started_elems`, `claim_ended`).
            ClientFrame::Claim { req, root, keys, ttl_ms, note, hold } => {
                if !self.features.contains(&super::live_watch::CLAIMS_FEATURE) {
                    return self.send(wire::error_frame(&wire::report("MPC-E113", "`claim` isn't taken here: this doc's socket doesn't list `claims` in welcome.features".to_string())));
                }
                let got = match root {
                    Some(root) => keys
                        .iter()
                        .map(|k| k2_mpc_base::elem::Key::new(k.as_str()).map_err(|e| SessionError::BadClaim(format!("{k:?} isn't a key: {e}"))))
                        .collect::<Result<Vec<_>, _>>()
                        .and_then(|keys| self.session.claim_elems(self.conn, &root, &keys, ttl_ms, note, hold)),
                    None => self.session.claim_elems_at(self.conn, &keys, ttl_ms, note, hold),
                };
                match got {
                    Ok(c) => self.send(wire::claimed_frame(req, &c.id.to_string())),
                    Err(e) => self.send(wire::presence_refused_frame(req, &e.report())),
                }
            }
            ClientFrame::Release { req, claim } => {
                if !self.features.contains(&super::live_watch::CLAIMS_FEATURE) {
                    return self.send(wire::error_frame(&wire::report("MPC-E113", "`release` isn't taken here: this doc's socket doesn't list `claims` in welcome.features".to_string())));
                }
                let id = match claim.as_deref().map(|c| k2_mpc_base::session::ClaimId::parse(c).ok_or(c)) {
                    None => None,
                    Some(Ok(id)) => Some(id),
                    Some(Err(c)) => {
                        let e = SessionError::BadClaim(format!("{c:?} isn't a claim id (claim-N)"));
                        return self.send(wire::presence_refused_frame(req, &e.report()));
                    }
                };
                match self.session.release_claim(self.conn, id) {
                    Ok(ids) => self.send(wire::released_frame(req, &ids.iter().map(|i| i.to_string()).collect::<Vec<_>>())),
                    Err(e) => self.send(wire::presence_refused_frame(req, &e.report())),
                }
            }
            // The app showed the removed-text notes: delivered, so no later
            // snapshot repeats them. No reply; a refusal (no read right) is
            // nothing to tell.
            ClientFrame::TakeNotes => {
                if let Err(e) = self.session.take_notes(self.conn) {
                    log_debug!("[docs/live] take_notes on {}: {e}", host::key_label(&self.key));
                }
                Ok(())
            }
            ClientFrame::View { root, first, last } => {
                let root = root.unwrap_or_else(|| ROOT.to_string());
                let _ = self.session.set_view(self.conn, LineRange { root, first, last });
                Ok(())
            }
        }
    }

    /// A presence frame of the family `feature`: taken only when the welcome
    /// listed it (else `error`, `MPC-E113`, as an unknown frame); the kit's
    /// refusal is `presence_refused {req, error}` and nothing changes.
    fn presence(&mut self, req: u64, feature: &'static str, set: impl FnOnce(&Session, ConnId) -> Result<(), SessionError>) -> Result<(), Stop> {
        if !self.features.contains(&feature) {
            let e = wire::report("MPC-E113", format!("`{feature}` isn't taken here: this doc's socket doesn't list it in welcome.features"));
            return self.send(wire::error_frame(&e));
        }
        match set(&*self.session, self.conn) {
            Ok(()) => Ok(()),
            Err(e) => self.send(wire::presence_refused_frame(req, &e.report())),
        }
    }

    fn edit(&mut self, op: u64, root: &str, base: u64, hunks: Vec<k2_mpc_base::text::Hunk>, group: Option<String>) -> Result<(), Stop> {
        // A person is never held back by an agent's claim (as `k2 doc`).
        let mut opts = EditOptions::new().policy(ClaimPolicy::Ignore);
        if let Some(g) = group.as_deref() {
            opts = opts.group(g);
        }
        if self.dead.load(Ordering::SeqCst) {
            return Err(Stop::Close(Closed::new(CloseReason::Left)));
        }
        match self.session.edit_at_base(self.conn, root, base, hunks, &opts) {
            Ok(r) => {
                let v = r.result.version;
                let applied = !r.hunks.is_empty();
                if applied {
                    self.own.insert(v);
                    // Every change before V goes out first; our own event at V
                    // is skipped (by version). It was queued during the commit;
                    // wait briefly in case it isn't there yet.
                    self.pump_upto(Some(v - 1))?;
                    // A change touching several roots is one event per root,
                    // all at V: the first may need the wait, the rest are queued.
                    let mut wait = true;
                    loop {
                        let ours = if wait {
                            matches!(self.peek_wait(OWN_EVENT_WAIT), Some(e) if e.version == v)
                        } else {
                            matches!(self.peek(), Some(e) if e.version == v)
                        };
                        if !ours {
                            break;
                        }
                        wait = false;
                        let e = self.peeked.take().expect("peeked");
                        self.forward(e)?;
                    }
                } else {
                    // Nothing applied: everything queued, skipping nothing.
                    self.pump()?;
                }
                self.seen = self.seen.max(v);
                self.send(wire::ack_frame(op, v, &r.hunks))
            }
            Err(e) => {
                // Refused: everything queued goes out first (WIRE.md v2).
                self.pump()?;
                self.send(wire::refused_frame(op, &e))
            }
        }
    }

    fn undo(&mut self, op: u64, undo: bool) -> Result<(), Stop> {
        let out = if undo { self.session.undo(self.conn, 1) } else { self.session.redo(self.conn, 1) };
        match out {
            Ok(o) => {
                let v = match &o {
                    k2_mpc_base::session::UndoOutcome::Applied { result, .. } => result.version,
                    _ => self.seen,
                };
                // The change arrives as `change` (never filtered: it isn't an
                // edit of this socket's), before the reply.
                self.pump_upto(Some(v))?;
                self.send(wire::undo_reply(op, &o, v))
            }
            Err(e) => self.send(wire::refused_frame(op, &e.report())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Live socket keys on the settings' own registry (one `core_of`).
    #[test]
    fn files_belong_to_their_core() {
        use super::super::cores::core_of;
        assert_eq!(core_of(Tier::of("notes.md")), "markdown");
        assert_eq!(core_of(Tier::of("main.rs")), "text");
        assert_eq!(core_of(Tier::of("photo.png")), "any-file");
    }

    /// Kit v0.20.0 (MPC-E169): a tab opening a doc a newer K2 put on the
    /// native engine gets one `closed` frame, declined (no retry), with
    /// K2's code, the kit's line and the next step, and the kit's report.
    #[test]
    fn a_native_doc_closes_the_tab_declined_with_its_own_code() {
        let c = open_error(&SessionError::NativeEngineMissing { stored: "native-stamp-1".into(), stored_format: 1 });
        assert_eq!(c.reason.as_str(), "declined");
        assert_eq!(c.code.as_deref(), Some("native_engine_missing"));
        let m = c.message.as_deref().expect("a message");
        assert!(m.starts_with("this doc is on the native engine (format 1), which this build doesn't have. "), "{m}");
        assert!(m.contains("update K2 to open it live again"), "{m}");
        assert_eq!(c.error.as_ref().expect("the kit's report").id(), "MPC-E169");
    }

    /// The file Rosson opened: a plain 10 KB HTML page in ANOTHER
    /// workspace's `.k2/agent/work/`. The owner's absolute path resolves to
    /// that workspace (a file there outside `.k2/` opens live), but `.k2/`
    /// outside its doc folders is K2's private folder: the socket closes
    /// `declined` with `protected` and the daemon's words naming the folder,
    /// never a size or text reason. Outside every workspace is its own code.
    #[test]
    fn a_page_in_another_workspaces_k2_folder_is_refused_as_protected_not_too_large() {
        let dir = crate::test_support::Scratch::new();
        let mine = dir.path().join("mine");
        let other = dir.path().join("other");
        std::fs::create_dir_all(&mine).expect("mine");
        std::fs::create_dir_all(other.join(".k2/agent/work")).expect("other's .k2/agent/work");
        let mut page = String::from("<!doctype html>\n<html><head><style>body { font: 14px sans-serif; }</style></head><body>\n");
        while page.len() < 9_900 {
            page.push_str("<p>The k5 CLI port, slice by slice.</p>\n");
        }
        page.push_str("</body></html>\n");
        let inside = other.join(".k2/agent/work/k5-cli-progress.html");
        std::fs::write(&inside, &page).expect("page in .k2");
        let beside = other.join("k5-cli-progress.html");
        std::fs::write(&beside, &page).expect("page beside .k2");
        let loose = dir.path().join("loose.html");
        std::fs::write(&loose, &page).expect("page in no workspace");
        let (mine_id, other_id) = (uuid::Uuid::new_v4().to_string(), uuid::Uuid::new_v4().to_string());
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            for (id, name, root) in [(&mine_id, "live-mine", &mine), (&other_id, "live-other", &other)] {
                let root = root.canonicalize().expect("root");
                conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)", rusqlite::params![id, name, root.to_string_lossy()])
                    .expect("register the workspace");
            }
        }
        let open = |p: &std::path::Path| {
            let params: HashMap<String, String> = [("doc".to_string(), p.to_string_lossy().to_string())].into();
            live_target(&super::super::caller::owner(), &params)
        };

        let c = open(&inside).expect_err("a page in .k2/agent/work is not a live doc");
        println!("[docs/live] refusal for {}: {}", inside.display(), c.value());
        assert_eq!(c.reason, CloseReason::Declined);
        assert_eq!(c.code.as_deref(), Some("protected"), "{:?}", c);
        assert_eq!(
            c.message.as_deref(),
            Some(".k2/agent/work/k5-cli-progress.html is in the workspace's private .k2 folder, where only Markdown and files under .k2/wiki, .k2/prds, .k2/docs or .k2/conflicts are live docs"),
        );
        assert!(c.error.is_none(), "K2's own rule, not a kit report: {:?}", c.error);
        let frame = c.value().to_string();
        assert!(!frame.contains("too_large") && !frame.contains("not_utf8") && !frame.contains("binary"), "{frame}");

        let t = open(&beside).expect("the same page outside .k2 opens live, in its own workspace");
        assert_eq!((t.project_id.as_str(), t.rel.as_str()), (other_id.as_str(), "k5-cli-progress.html"));
        assert_eq!(t.tier, Tier::Text(k2_mpc_text::Family::Markup));

        let c = open(&loose).expect_err("a page in no workspace");
        assert_eq!((c.reason, c.code.as_deref()), (CloseReason::NotFound, Some("workspace_not_found")));
        assert_eq!(c.message.as_deref(), Some("that file is not inside a workspace on this server"));

        let db = k2_core::db::shared();
        let conn = db.lock();
        for id in [&mine_id, &other_id] {
            conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![id]).expect("unregister");
        }
    }

    #[test]
    fn people_only_and_never_an_app_pass_or_an_agent() {
        use crate::sidecar_routes::Caller;
        assert_eq!(person_for(Caller::Owner, false).ok().expect("owner").pid(), "person:owner");
        let code = |r: Result<DocCaller, CliResponse>| {
            let r = r.expect_err("refused");
            assert_eq!(r.status, "403 Forbidden");
            serde_json::from_str::<serde_json::Value>(&r.body).expect("json")["error"]["code"].as_str().expect("code").to_string()
        };
        assert_eq!(code(person_for(Caller::Owner, true)), "app_docs_not_available");
        assert_eq!(code(person_for(Caller::Shell { project_id: "p".into() }, false)), "agents_use_k2_doc");
        assert_eq!(code(person_for(Caller::WrongCredential, false)), "wrong_credential");
        assert_eq!(code(person_for(Caller::Invalid, false)), "forbidden");
    }

    /// One doc ("one\ntwo\n") on the vendored kit, a live window of 2
    /// changes, and two people: Ada (who edits) and Bob (whose carets come in).
    struct Carets {
        dir: std::path::PathBuf,
        s: Arc<Session>,
        ada: ConnId,
        bob: ConnId,
        _mgr: k2_mpc_base::session::SessionManager,
    }

    impl Drop for Carets {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    struct CaretHost;

    impl k2_mpc_base::session::Host for CaretHost {
        fn rights(&self, _who: &k2_mpc_base::session::Participant, _key: &DocKey) -> k2_mpc_base::session::Rights {
            k2_mpc_base::session::Rights::editor()
        }
        fn now_ms(&self) -> u64 {
            1_700_000_000_000
        }
        fn on_event(&self, _ev: &k2_mpc_base::session::AuditEvent) {}
    }

    impl Carets {
        fn new() -> Carets {
            use k2_mpc_base::session::{Config, Participant, ParticipantKind, SessionManager, TextFilePack};
            let dir = std::env::temp_dir().join(format!("k2-live-carets-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).expect("dir");
            let mut config = Config::default();
            config.live_window_events = 2;
            config.edit_rate_per_s = 1_000_000;
            config.edit_burst = 1_000_000;
            let store = Arc::new(k2_mpc_base::store::DocStore::open(dir.join("docs.redb")).expect("store"));
            let mgr = SessionManager::new(store, Arc::new(CaretHost), config);
            let s = mgr
                .open(
                    Arc::new(TextFilePack::new("markdown", 4 << 20)),
                    &DocKey { space: "ws".into(), path: "notes.md".into() },
                    Some(b"one\ntwo\n"),
                )
                .expect("open");
            let ada = s.join(Participant::new("ada", "Ada", "#7c3aed", ParticipantKind::Person, "tab"), Stay::Held).expect("join ada").conn;
            let bob = s.join(Participant::new("bob", "Bob", "#16a34a", ParticipantKind::Person, "tab"), Stay::Held).expect("join bob").conn;
            Carets { dir, s, ada, bob, _mgr: mgr }
        }
        fn version(&self) -> Seq {
            self.s.status().expect("status").version
        }
        /// Ada's edit at the doc's version; the version after it.
        fn ada_edits(&self, start: usize, end: usize, insert: &str) -> Seq {
            let hunk = k2_mpc_base::text::Hunk { start, end, insert: insert.into() };
            let r = self.s.edit_at_base(self.ada, ROOT, self.version(), vec![hunk], &EditOptions::new()).expect("ada's edit");
            assert!(!r.hunks.is_empty(), "ada's edit applied nothing");
            r.result.version
        }
        /// Bob's caret as the kit's roster has it, in bytes.
        fn bobs_caret(&self) -> Option<(usize, usize)> {
            let roster = self.s.roster(self.bob).expect("roster");
            let bob = roster.iter().find(|e| e.participant.pid == "bob").expect("bob on the roster");
            bob.cursor.as_ref().map(|c| {
                assert_eq!(c.cursor.root, ROOT, "bob's caret root");
                (c.cursor.anchor, c.cursor.head)
            })
        }
    }

    /// WIRE.md "Carets": a caret one change behind (the change was still on
    /// its way to the client) is carried through it, `Bias::After`.
    #[test]
    fn a_caret_behind_is_mapped_through_the_changes_since() {
        let f = Carets::new();
        let v0 = f.version();
        // "one\ntwo\n" -> "XXone\ntwo\n"
        let v1 = f.ada_edits(0, 0, "XX");
        assert_eq!(v1, v0 + 1);
        // Bob selected "two" (bytes 4..7) at v0; this socket had sent v1.
        assert_eq!(place_caret(&f.s, f.bob, ROOT, v0, v1, 4, 7), Some((6, 9)));
        assert_eq!(f.bobs_caret(), Some((6, 9)), "the kit holds the mapped caret");
        // A caret exactly at Ada's insertion goes after it (Bias::After).
        assert_eq!(place_caret(&f.s, f.bob, ROOT, v0, v1, 0, 0), Some((2, 2)));
        assert_eq!(f.bobs_caret(), Some((2, 2)));
        // Two changes behind, a deletion among them: "XXone\ntwo\n" -> "XXne\ntwo\n".
        let v2 = f.ada_edits(2, 3, "");
        assert_eq!(place_caret(&f.s, f.bob, ROOT, v0, v2, 4, 7), Some((5, 8)));
        assert_eq!(f.bobs_caret(), Some((5, 8)));
    }

    #[test]
    fn a_caret_at_the_docs_version_is_set_as_it_is() {
        let f = Carets::new();
        let v1 = f.ada_edits(0, 0, "XX");
        assert_eq!(place_caret(&f.s, f.bob, ROOT, v1, v1, 1, 3), Some((1, 3)));
        assert_eq!(f.bobs_caret(), Some((1, 3)));
    }

    /// Past the live window (2 changes here) `events_since` refuses: dropped,
    /// and the caret the kit already held stays.
    #[test]
    fn a_caret_past_the_live_window_is_dropped() {
        let f = Carets::new();
        let v0 = f.version();
        assert_eq!(place_caret(&f.s, f.bob, ROOT, v0, v0, 1, 1), Some((1, 1)));
        // Three appends at the end: Bob's held caret at 1 doesn't move.
        f.ada_edits(8, 8, "a");
        f.ada_edits(9, 9, "b");
        let v3 = f.ada_edits(10, 10, "c");
        assert_eq!(v3, v0 + 3);
        assert!(f.s.events_since(f.bob, v0).is_err(), "the kit's window should refuse 3 changes back");
        assert_eq!(place_caret(&f.s, f.bob, ROOT, v0, v3, 4, 7), None);
        assert_eq!(f.bobs_caret(), Some((1, 1)), "a dropped caret leaves the held one");
    }

    /// A caret at a version this socket never told the client: dropped,
    /// whether the doc has reached it or not.
    #[test]
    fn a_caret_from_the_future_is_dropped() {
        let f = Carets::new();
        let v0 = f.version();
        assert_eq!(place_caret(&f.s, f.bob, ROOT, v0, v0, 2, 2), Some((2, 2)));
        // The doc is at v1 but this socket had told the client only v0.
        let v1 = f.ada_edits(8, 8, "z");
        assert_eq!(place_caret(&f.s, f.bob, ROOT, v1, v0, 4, 7), None);
        // A version the doc hasn't reached.
        assert_eq!(place_caret(&f.s, f.bob, ROOT, v1 + 1, v1 + 1, 4, 7), None);
        assert_eq!(f.bobs_caret(), Some((2, 2)), "a dropped caret leaves the held one");
    }

    #[test]
    fn a_caret_on_another_root_is_dropped() {
        let f = Carets::new();
        let v0 = f.version();
        assert_eq!(place_caret(&f.s, f.bob, "other", v0, v0, 0, 0), None);
        assert_eq!(f.bobs_caret(), None);
    }

    #[test]
    fn the_keepalive_is_20_seconds_by_default() {
        if std::env::var_os("K2_DOCS_LIVE_PING_MS").is_none() {
            assert_eq!(ping_interval(), Duration::from_secs(20));
        }
    }
}
