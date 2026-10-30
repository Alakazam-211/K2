//! `WS /cli/docs/live`: the Live view's socket (plan L2). One socket is one
//! person's tab on one text doc, joined to the kit's session; the editor
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
//! of the Markdown and text packs), `&resume=<version>` on a reconnect.
//!
//! **The contract is the kit's `client/WIRE.md` v2** (kit main `bea1885`;
//! frames in [`super::live_wire`], pinned against the kit's golden JSON).
//!
//! **Per socket, in order** (WIRE.md "Connecting"): resolve and jail the
//! file, check its core is on and the person may read it, `open_file`,
//! `join(Stay::Held)` (a new connection and client id, also on every
//! reconnect), `subscribe` and `subscribe_roster` **before** the read, then
//! `welcome {root, version, text?, client, self, rights}`, the first frame.
//! `text` is left out on a resume the window still covers. Anything refused
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
//! - `cursor` → `set_cursor` only when its version is the doc's version as
//!   this socket last saw it; behind, it is dropped (WIRE.md "Carets" allows
//!   dropping; the client re-sends after its next change). No reply; a
//!   refused caret is ignored.
//! - `visible` (and the Thread's `{"t":"viewing","visible"}`) →
//!   `set_visible`; `view` → `set_view`; `hello` is ignored (the URL carries
//!   root and resume).
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
//! file's core is switched off ([`close_core`], or seen within a second
//! through the gate): write back now (`flush`), `leave`, then
//! `closed {reason:"closed", code:"core_off", core, version, written}` and
//! close code 4010.
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
use k2_mpc_base::session::position::{At, LineRange};
use k2_mpc_base::session::{
    ChangeEvent, ClaimPolicy, ConnId, EditOptions, RosterEvent, Session, SessionError, Stay,
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

/// The multiplayer core a file belongs to (PRD §4.1): Markdown →
/// `markdown`, any other text → `text`, any other file → `any-file`.
pub fn core_of(tier: Tier) -> &'static str {
    match tier {
        Tier::Markdown => "markdown",
        Tier::Text(_) => "text",
        Tier::Blob => "any-file",
    }
}

/// The core gate: whether a core is on. **Seam for the settings slice
/// (S1/S2):** it registers its in-process mirror (`docs/cores.rs`) here at
/// boot. Until something registers, every core counts as on, so the Live
/// view works on its own.
pub type CoreGate = fn(&str) -> bool;

fn gate() -> &'static RwLock<Option<CoreGate>> {
    static G: OnceLock<RwLock<Option<CoreGate>>> = OnceLock::new();
    G.get_or_init(|| RwLock::new(None))
}

/// Register the core gate (`None` takes it away: every core on).
pub fn set_core_gate(g: Option<CoreGate>) {
    *gate().write().unwrap_or_else(|p| p.into_inner()) = g;
}

/// Whether `core` is on now.
pub fn core_is_on(core: &str) -> bool {
    match *gate().read().unwrap_or_else(|p| p.into_inner()) {
        Some(g) => g(core),
        None => true,
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

enum Ctl {
    /// A reconnect from the same tab retired this connection (`prev`).
    Replaced,
    /// The core was switched off: write back, leave, `closed`.
    CoreOff { by: Option<String> },
    /// The doc was renamed (`k2 doc mv`): `closed {reason: "moved", path}`.
    Moved { path: String },
}

fn sockets() -> &'static Mutex<HashMap<u64, Handle>> {
    static S: OnceLock<Mutex<HashMap<u64, Handle>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_socket_id() -> u64 {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

/// **Seam for the settings slice (S2):** after a core is switched off (the
/// gate already says off), end every Live socket on its files. Each writes
/// its doc back, leaves, and gets `closed {reason:"closed", code:"core_off",
/// core, version, written}` then close code 4010. `by` (a display name) goes
/// in the frame's message. Returns how many sockets were told. The sockets
/// also notice a switched-off core on their own within a second.
pub fn close_core(core: &str, by: Option<&str>) -> usize {
    let s = sockets().lock().unwrap_or_else(|p| p.into_inner());
    let mut n = 0;
    for h in s.values().filter(|h| h.core == core) {
        if h.ctl.send(Ctl::CoreOff { by: by.map(str::to_string) }).is_ok() {
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
            "it was opened as an any-file doc earlier (its text was refused then), and stays one",
        ),
        SessionError::Doc(DocError::Quarantined) => Closed {
            code: Some("doc_quarantined".into()),
            message: Some(super::routes::QUARANTINED_HINT.to_string()),
            ..Closed::kit(CloseReason::Declined, e)
        },
        SessionError::NotFound(_) => Closed::kit(CloseReason::NotFound, e),
        SessionError::Forbidden { .. } => Closed::kit(CloseReason::Auth, e),
        SessionError::Released => restart(),
        SessionError::Deleted => Closed::kit(CloseReason::Deleted, e),
        _ => Closed::kit(CloseReason::Declined, e),
    }
}

/// Resolve, gate, open, join, subscribe, read. Blocking.
fn open_live(caller: &DocCaller, params: &HashMap<String, String>) -> Result<Opened, Closed> {
    let rt = super::runtime().map_err(|u| match &u {
        super::Unavailable::Failed(_) => Closed::k2(CloseReason::Declined, "docs_unavailable", u.message()),
        _ => restart(),
    })?;
    let root = params.get("root").map(|r| r.trim()).filter(|r| !r.is_empty()).unwrap_or(ROOT);
    if root != ROOT {
        return Err(Closed::k2(CloseReason::Declined, "bad_request", format!("root must be `{ROOT}`")));
    }
    let resume = match params.get("resume").map(|r| r.trim()).filter(|r| !r.is_empty()) {
        Some(r) => Some(
            r.parse::<u64>()
                .map_err(|_| Closed::k2(CloseReason::Declined, "bad_request", "resume must be a whole number"))?,
        ),
        None => None,
    };
    // A malformed `prev` is ignored like an unknown one.
    let prev = params.get("prev").and_then(|v| v.trim().parse::<u64>().ok());
    let path = params.get("doc").or_else(|| params.get("path")).cloned().unwrap_or_default();
    let t: Target = target::resolve(params.get("workspace").map(String::as_str), &path, caller.default_workspace())
        .map_err(|r| Closed::k2(target_reason(&r), r.code, r.message))?;
    let core = core_of(t.tier);
    if t.tier == Tier::Blob {
        return Err(Closed::k2(
            CloseReason::Declined,
            "not_a_text_doc",
            format!("{} isn't a text file; the Live view edits Markdown and other text", t.rel),
        ));
    }
    if !core_is_on(core) {
        return Err(Closed {
            core: Some(core.to_string()),
            ..Closed::k2(CloseReason::Closed, "core_off", format!("the {core} core is off on this server: open the file the usual way"))
        });
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
    let mut session = super::routes::open(&rt, &t).map_err(|e| open_error(&e))?;
    let joined = match session.join(participant.clone(), Stay::Held) {
        Err(SessionError::Unloaded) => {
            session = super::routes::open(&rt, &t).map_err(|e| open_error(&e))?;
            session.join(participant, Stay::Held)
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
    let changes = session.subscribe(conn).map_err(|e| fail(&session, e))?;
    let roster = session.subscribe_roster(conn).map_err(|e| fail(&session, e))?;
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
    let welcome = wire::welcome_frame(
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
    Ok(Opened { session, conn, pid, key, core, client: joined.client, version: read.version, changes, roster, welcome })
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
pub async fn serve(stream: &mut TcpStream, params: HashMap<String, String>, caller: DocCaller) {
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
    let worker = Worker::new(opened, in_rx, ctl_rx, out_tx, dead);
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
    /// Versions of this connection's own edits not yet passed: their events
    /// are never forwarded (G3).
    own: HashSet<Seq>,
    /// The doc's version as this socket last told the client.
    seen: Seq,
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
            own: HashSet::new(),
            seen: o.version,
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
        loop {
            if let Err(s) = self.pump() {
                return s;
            }
            if self.dead.load(Ordering::SeqCst) {
                return Stop::Close(Closed::new(CloseReason::Left));
            }
            match self.ctl.try_recv() {
                Ok(Ctl::Replaced) => return Stop::Close(Closed::new(CloseReason::Left)),
                Ok(Ctl::CoreOff { by }) => return Stop::Close(self.core_off(by)),
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
                    return Stop::Close(self.core_off(None));
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
        if self.own.contains(&e.version) {
            return Ok(());
        }
        self.own.retain(|v| *v >= e.version);
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
        loop {
            match self.roster.try_recv() {
                Ok(ev) => {
                    if let Some(v) = wire::roster_event(&ev) {
                        self.send(wire::roster_frame(v))?;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Err(Stop::Close(self.why_gone())),
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

    /// The kit dropped this connection's queue: why.
    fn why_gone(&self) -> Closed {
        if self.dead.load(Ordering::SeqCst) {
            // A reconnect retired it (`prev`); the tab moved to the new socket.
            return Closed::new(CloseReason::Left);
        }
        if !core_is_on(self.core) {
            return self.core_off(None);
        }
        if let Err(super::Unavailable::Released) = super::runtime() {
            return restart();
        }
        match self.session.touch(self.conn) {
            // Still joined: the queue overflowed (1,024 events behind).
            Ok(()) => Closed::k2(CloseReason::CutOff, "cut_off", "this socket fell behind the doc; reconnect and resume"),
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

    /// The core was switched off: write back now, then the frame.
    fn core_off(&self, by: Option<String>) -> Closed {
        let written = self.session.flush(self.conn).is_ok();
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

    fn on_frame(&mut self, text: &str) -> Result<(), Stop> {
        let f = match wire::parse(text) {
            Ok(f) => f,
            Err(b) => return self.send(b.reply()),
        };
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
                // G6: the kit places a caret in the current text. Only a caret
                // at the version this socket last told the client goes in.
                self.pump()?;
                if version == self.seen && root == ROOT {
                    if let Err(e) = self.session.set_cursor(self.conn, &root, At::Byte(anchor), At::Byte(head)) {
                        log_debug!("[docs/live] caret ignored: {}", e.report().plain());
                    }
                }
                Ok(())
            }
            ClientFrame::Visible(v) => {
                let _ = self.session.set_visible(self.conn, v);
                Ok(())
            }
            ClientFrame::Hello => Ok(()),
            ClientFrame::View { root, first, last } => {
                let root = root.unwrap_or_else(|| ROOT.to_string());
                let _ = self.session.set_view(self.conn, LineRange { root, first, last });
                Ok(())
            }
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

    #[test]
    fn files_belong_to_their_core() {
        assert_eq!(core_of(Tier::of("notes.md")), "markdown");
        assert_eq!(core_of(Tier::of("main.rs")), "text");
        assert_eq!(core_of(Tier::of("photo.png")), "any-file");
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

    #[test]
    fn the_keepalive_is_20_seconds_by_default() {
        if std::env::var_os("K2_DOCS_LIVE_PING_MS").is_none() {
            assert_eq!(ping_interval(), Duration::from_secs(20));
        }
    }
}
