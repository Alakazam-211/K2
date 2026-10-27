//! K2 compute nodes: the controller's live side (`prd-k2-compute-nodes-v1`
//! §6.2, §7, §11.5).
//!
//! - `GET /cli/compute/enroll` (WebSocket): a node presents the binding of
//!   a one-time code and its public key; we pick the enroll nonce AFTER
//!   that, sign it, check the node's proof and store a PENDING row with
//!   the nonce SAS (CN1). A human confirms on this server.
//! - `GET /cli/compute/attach` (WebSocket): mutual signed handshake (CN2),
//!   then every frame both ways is a signed, sequenced envelope. No HTTP
//!   credential: both routes are `Public` rows whose handlers fail closed
//!   (CN11, CN19). The node is never a token principal.
//!
//! The engine keeps one live connection per node, places queued jobs
//! with the pure fair queue (`k2_core::compute::scheduler`), relays logs
//! into the controller's log store, reconciles after reconnects (node =
//! truth for execution, controller = truth for intent, no blind replay),
//! and messages the requesting session when a detached job ends (CN6).
//!
//! Locking: the engine mutex is only ever held for in-memory work, never
//! across a DB call; `KICK` serializes placements and is taken before
//! the DB lock.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use k2_core::compute::proto::crypto::{self, SigningKey};
use k2_core::compute::proto::frames::{
    self, ArtifactChunk, ArtifactFailed, ArtifactGet, Assign, Cancel, Challenge, Direction, EnrollChallenge, EnrollDone,
    Fence, Frame, HandshakeFrame, JobState, JobsSnapshot, LogChunk, Need, Offer, ReceiptFrame, Refused, ResumeHint,
    Revoked, StateUpdate, Transfer, TransferFailed, TransferKind, Welcome,
};
use k2_core::compute::proto::results::{self, ArtifactInfo, Outputs};
use k2_core::compute::proto::pairing;
use k2_core::compute::proto::share::{self as pshare, QueueView};
use k2_core::compute::proto::tiers::{self, Tier};
use k2_core::compute::share::{self, pending};
use k2_core::compute::{self as compute, logs, scheduler, store};
use k2_core::log_debug;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::routes::dispatcher::Ingress;

pub const ATTACH_PATH: &str = "/cli/compute/attach";
pub const ENROLL_PATH: &str = "/cli/compute/enroll";

/// What a connection's writer sends.
enum Out {
    Frame(Box<Frame>),
    Close,
}

struct Conn {
    conn_id: u64,
    tx: mpsc::UnboundedSender<Out>,
    route: &'static str,
    offer: Option<Offer>,
    bytes_in: Arc<AtomicU64>,
    bytes_out: Arc<AtomicU64>,
}

#[derive(Default)]
struct Engine {
    next_conn: u64,
    conns: HashMap<String, Conn>,
    last_ws: HashMap<String, String>,
}

static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);
static KICK: Mutex<()> = Mutex::new(());
/// The ticker task. Restarted when its runtime is gone (a test's
/// runtime, or never started).
static TICKER: Mutex<Option<tokio::task::JoinHandle<()>>> = Mutex::new(None);

fn with_engine<T>(f: impl FnOnce(&mut Engine) -> T) -> T {
    let mut g = ENGINE.lock().unwrap_or_else(|p| p.into_inner());
    let e = g.get_or_insert_with(Engine::default);
    // A session whose task is gone (its writer queue closed) is offline,
    // even if its cleanup never ran (a dropped runtime): never "online"
    // with a dead queue, never a placement into it.
    e.conns.retain(|_, c| !c.tx.is_closed());
    f(e)
}

/// Wakes log/state long-pollers (`/cli/compute/jobs/logs?wait=`).
static CHANGE: (Mutex<u64>, Condvar) = (Mutex::new(0), Condvar::new());

/// When each job's log was last polled (an attached CLI follows every
/// ≤ 25 s); in memory only.
static FOLLOWERS: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

/// A follower just asked for `job_id`'s log.
pub fn touch_follower(job_id: &str) {
    let mut g = FOLLOWERS.lock().unwrap_or_else(|p| p.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    if m.len() > 4096 {
        m.retain(|_, t| t.elapsed() < Duration::from_secs(600));
    }
    m.insert(job_id.to_string(), Instant::now());
}

/// Someone followed this job's log in the last 45 s.
fn follower_recent(job_id: &str) -> bool {
    FOLLOWERS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .and_then(|m| m.get(job_id))
        .is_some_and(|t| t.elapsed() < Duration::from_secs(45))
}

pub fn notify_change() {
    let (m, cv) = &CHANGE;
    let mut g = m.lock().unwrap_or_else(|p| p.into_inner());
    *g = g.wrapping_add(1);
    cv.notify_all();
}

/// Current change counter.
pub fn change_counter() -> u64 {
    *CHANGE.0.lock().unwrap_or_else(|p| p.into_inner())
}

/// Block until the change counter moves past `seen` or `timeout` passes.
pub fn wait_change(seen: u64, timeout: Duration) -> u64 {
    let (m, cv) = &CHANGE;
    let g = m.lock().unwrap_or_else(|p| p.into_inner());
    let (g, _) = cv
        .wait_timeout_while(g, timeout, |c| *c == seen)
        .unwrap_or_else(|p| p.into_inner());
    *g
}

/// Is `node_id` connected right now?
pub fn is_online(node_id: &str) -> bool {
    with_engine(|e| e.conns.contains_key(node_id))
}

/// `(online, route, live offer)` for display.
pub fn live_view(node_id: &str) -> (bool, Option<&'static str>, Option<Offer>) {
    with_engine(|e| match e.conns.get(node_id) {
        Some(c) => (true, Some(c.route), c.offer.clone()),
        None => (false, None, None),
    })
}

fn route_label(ingress: Ingress) -> &'static str {
    match ingress {
        Ingress::Tunnel => "relay",
        Ingress::Lan => "lan",
        Ingress::Loopback => "loopback",
        // An App's gateway link never serves the node socket (its gate).
        Ingress::App => "app-link",
    }
}

fn day() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// This server's label for nodes (`rosson.k2.dev`), or the host name.
pub fn controller_label() -> String {
    match k2_core::tunnel::config::load() {
        Ok(cfg) if !cfg.subdomain.trim().is_empty() => {
            let s = cfg.subdomain.trim();
            if s.contains('.') {
                s.to_string()
            } else {
                format!("{s}.k2.dev")
            }
        }
        _ => hostname(),
    }
}

fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "this server".to_string())
}

// ── shared socket helpers ────────────────────────────────────────────

type Ws<'a> = tokio_tungstenite::WebSocketStream<&'a mut TcpStream>;

async fn send_hs(ws: &mut Ws<'_>, f: &HandshakeFrame) -> bool {
    match serde_json::to_string(f) {
        Ok(t) => ws.send(Message::Text(t)).await.is_ok(),
        Err(_) => false,
    }
}

async fn refuse(ws: &mut Ws<'_>, code: &str, message: &str) {
    let _ = send_hs(ws, &HandshakeFrame::Refused(Refused { code: code.into(), message: message.into() })).await;
    let _ = ws.close(None).await;
}

/// Next text message before `deadline`, as a handshake frame.
async fn recv_hs(ws: &mut Ws<'_>, deadline: tokio::time::Instant) -> Option<HandshakeFrame> {
    loop {
        let msg = tokio::time::timeout_at(deadline, ws.next()).await.ok()??.ok()?;
        match msg {
            Message::Text(t) => {
                if t.len() > frames::MAX_FRAME_BYTES {
                    return None;
                }
                return serde_json::from_str(&t).ok();
            }
            Message::Ping(_) | Message::Pong(_) => continue,
            _ => return None,
        }
    }
}

fn audit(actor: &str, kind: &str, node: Option<&str>, detail: serde_json::Value) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::record_event(&conn, actor, kind, node, None, None, detail);
}

// ── enroll ───────────────────────────────────────────────────────────

/// `GET /cli/compute/enroll`. Caller already checked `compute::enabled()`.
pub async fn serve_enroll(stream: &mut TcpStream, ingress: Ingress) {
    let mut ws = match tokio_tungstenite::accept_async(&mut *stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[compute] enroll handshake failed: {e}");
            return;
        }
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(frames::HANDSHAKE_SECS);
    let Some(HandshakeFrame::EnrollRequest(req)) = recv_hs(&mut ws, deadline).await else {
        return refuse(&mut ws, "protocol_error", "expected enroll_request").await;
    };
    if req.protocol < frames::MIN_PROTOCOL {
        return refuse(&mut ws, "node_upgrade_required", "this node is too old for this server; update k2-node").await;
    }
    let node_fp = match crypto::fingerprint_of_spki_pem(&req.node_public_key_pem) {
        Ok(fp) => fp,
        Err(_) => return refuse(&mut ws, "bad_key", "the node key is not a P-256 public key").await,
    };
    let node_spki = match crypto::pem_decode("PUBLIC KEY", &req.node_public_key_pem) {
        Ok(d) => d,
        Err(_) => return refuse(&mut ws, "bad_key", "the node key is unreadable").await,
    };
    let mut binding = req.code_binding.trim().to_ascii_lowercase();
    let (entry, via) = if let Some(ticket) = req.ticket.as_deref() {
        // One-click (same Connect account): the control plane's signed
        // ticket stands in for the code. Still only `pending`.
        let own = k2_core::tunnel::config::load().map(|c| c.subdomain.trim().to_ascii_lowercase()).unwrap_or_default();
        match compute::ticket::verify(ticket, &own, &node_fp, &req.name, compute::now()) {
            Ok(t) => {
                let tb = t.binding();
                if !crypto::ct_eq(tb.as_bytes(), binding.as_bytes()) {
                    return refuse(&mut ws, "protocol_error", "code_binding must be the ticket's binding").await;
                }
                binding = tb;
                (
                    compute::enroll::CodeEntry {
                        binding: binding.clone(),
                        name: t.node_name.clone(),
                        minted_by: format!("ticket:{}", t.kid),
                        expires_at: t.exp,
                    },
                    "ticket",
                )
            }
            Err(r) => {
                audit("node", "enroll_refused", None, serde_json::json!({"reason": r.code(), "detail": r.message(), "route": route_label(ingress), "fp": node_fp}));
                return refuse(&mut ws, r.code(), &r.message()).await;
            }
        }
    } else {
        match compute::enroll::take(&binding) {
            Ok(e) => (e, "code"),
            Err(r) => {
                audit("node", "enroll_refused", None, serde_json::json!({"reason": r.code(), "route": route_label(ingress), "fp": node_fp}));
                let msg = match r {
                    compute::enroll::CodeRefusal::LockedOut => "too many wrong codes: every waiting code was voided; ask the owner for a new one",
                    compute::enroll::CodeRefusal::Invalid => "the enroll code is wrong, used, or expired",
                };
                return refuse(&mut ws, r.code(), msg).await;
            }
        }
    };
    let key = match compute::controller_key() {
        Ok(k) => k,
        Err(e) => return refuse(&mut ws, "controller_key", &format!("the server can't load its key: {e}")).await,
    };
    let controller_fp = key.fingerprint();
    let (nonce, node_id) = match (crypto::random_bytes(16), crypto::random_id()) {
        (Ok(n), Ok(id)) => (crypto::b64(&n), id),
        _ => return refuse(&mut ws, "internal", "random failed").await,
    };
    let sig = match key.sign_domain(
        frames::DOMAIN_ENROLL_CONTROLLER,
        &[node_fp.as_bytes(), nonce.as_bytes(), binding.as_bytes(), controller_fp.as_bytes()],
    ) {
        Ok(s) => s,
        Err(e) => return refuse(&mut ws, "internal", &e).await,
    };
    let challenge = HandshakeFrame::EnrollChallenge(EnrollChallenge {
        controller_public_key_pem: key.spki_pem(),
        enroll_nonce: nonce.clone(),
        node_id: node_id.clone(),
        sig,
    });
    if !send_hs(&mut ws, &challenge).await {
        return;
    }
    let Some(HandshakeFrame::EnrollProof(proof)) = recv_hs(&mut ws, deadline).await else {
        return refuse(&mut ws, "protocol_error", "expected enroll_proof").await;
    };
    if !crypto::verify_domain(
        &node_spki,
        frames::DOMAIN_ENROLL_NODE,
        &[controller_fp.as_bytes(), nonce.as_bytes(), binding.as_bytes(), node_fp.as_bytes()],
        &proof.sig,
    ) {
        audit("node", "enroll_refused", None, serde_json::json!({"reason": "bad_proof", "fp": node_fp}));
        return refuse(&mut ws, "bad_proof", "the node didn't prove it holds its key").await;
    }
    let sas = pairing::compute_sas(&controller_fp, &node_fp, &nonce, &binding);
    let mut labels: BTreeMap<String, String> = req
        .labels
        .iter()
        .filter(|(k, v)| k.len() <= 32 && v.len() <= 64)
        .take(32)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    labels.insert("name".into(), entry.name.clone());
    let inserted = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let r = store::insert_pending_node_via(
            &conn,
            &node_id,
            &entry.name,
            &node_fp,
            &req.node_public_key_pem,
            &labels,
            &entry.minted_by,
            &sas,
            compute::now(),
            via,
        );
        if r.is_ok() {
            // Another server's `servers.add` enrolled this node with us
            // (protocol 3): this server is a tenant there, "shared by …".
            let shared_by = req.shared_by.as_deref().map(str::trim).filter(|l| pshare::valid_label(l));
            if let Some(owner) = shared_by {
                let _ = store::set_tenant_of(&conn, &node_id, owner);
            }
            store::record_event(
                &conn,
                &entry.minted_by,
                "enroll",
                Some(&node_id),
                None,
                None,
                serde_json::json!({"name": entry.name, "fp": node_fp, "route": route_label(ingress), "version": req.node_version, "via": via,
                    "sharedBy": shared_by}),
            );
        }
        r
    };
    if let Err(e) = inserted {
        let code = if e.contains("fingerprint") { "already_enrolled" } else { "name_taken" };
        // The node proved the code and its key; only this server's old row
        // for it is in the way. The code stays good (until its own expiry)
        // so the node's retry works once the owner removes that row.
        if code == "already_enrolled" && via == "code" {
            compute::enroll::restore(entry.clone());
        }
        return refuse(&mut ws, code, "a live node already has this name or key; remove it first").await;
    }
    k2_core::agent_hooks::emit(
        k2_core::agent_hooks::HookEvent::SyncSettings,
        serde_json::json!({"compute": "node_pending", "name": entry.name}),
    );
    let _ = send_hs(
        &mut ws,
        &HandshakeFrame::EnrollDone(EnrollDone { node_id, name: entry.name, state: "pending".into() }),
    )
    .await;
    let _ = ws.close(None).await;
}

// ── attach ───────────────────────────────────────────────────────────

/// `GET /cli/compute/attach`. Caller already checked `compute::enabled()`.
pub async fn serve_attach(stream: &mut TcpStream, ingress: Ingress) {
    ensure_ticker();
    let mut ws = match tokio_tungstenite::accept_async(&mut *stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[compute] attach handshake failed: {e}");
            return;
        }
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(frames::HANDSHAKE_SECS);
    let Some(HandshakeFrame::Hello(hello)) = recv_hs(&mut ws, deadline).await else {
        return refuse(&mut ws, "protocol_error", "expected hello").await;
    };
    if hello.protocol < frames::MIN_PROTOCOL {
        return refuse(&mut ws, "node_upgrade_required", "update K2 on this computer (k2-node is too old)").await;
    }
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::latest_node_by_fp(&conn, &hello.node_fp)
    };
    let Some(row) = row else {
        return refuse(&mut ws, "unknown_node", "this server doesn't know this node; enroll it again").await;
    };
    let Ok(node_spki) = crypto::pem_decode("PUBLIC KEY", &row.public_key_pem) else {
        return refuse(&mut ws, "unknown_node", "stored key unreadable").await;
    };
    let key = match compute::controller_key() {
        Ok(k) => k,
        Err(e) => return refuse(&mut ws, "controller_key", &e).await,
    };
    let controller_fp = key.fingerprint();
    let Ok(nonce_c) = crypto::nonce_b64() else { return };
    let Ok(sig) = key.sign_domain(
        frames::DOMAIN_ATTACH_CONTROLLER,
        &[hello.nonce_n.as_bytes(), hello.node_fp.as_bytes(), nonce_c.as_bytes()],
    ) else {
        return;
    };
    let ch = HandshakeFrame::Challenge(Challenge { controller_fp: controller_fp.clone(), nonce_c: nonce_c.clone(), sig, protocol: frames::PROTOCOL });
    if !send_hs(&mut ws, &ch).await {
        return;
    }
    let Some(HandshakeFrame::Proof(proof)) = recv_hs(&mut ws, deadline).await else {
        return refuse(&mut ws, "protocol_error", "expected proof").await;
    };
    if !crypto::verify_domain(
        &node_spki,
        frames::DOMAIN_ATTACH_NODE,
        &[nonce_c.as_bytes(), controller_fp.as_bytes(), hello.nonce_n.as_bytes()],
        &proof.sig,
    ) {
        audit("node", "attach_refused", Some(&row.id), serde_json::json!({"reason": "bad_proof", "route": route_label(ingress)}));
        return refuse(&mut ws, "bad_proof", "handshake signature didn't verify").await;
    }
    match row.state.as_str() {
        "revoked" => return refuse(&mut ws, "revoked", &format!("removed by {}", controller_label())).await,
        "pending" => {
            return refuse(
                &mut ws,
                "pending_confirmation",
                &format!("waiting for the owner to confirm the code on {}", controller_label()),
            )
            .await
        }
        _ => {}
    }
    let session = frames::session_id(&hello.nonce_n, &nonce_c);
    run_session(ws, key, node_spki, session, row, hello, ingress).await;
}

#[allow(clippy::too_many_arguments)]
async fn run_session(
    ws: Ws<'_>,
    key: SigningKey,
    node_spki: Vec<u8>,
    session: String,
    row: store::NodeRow,
    hello: frames::Hello,
    ingress: Ingress,
) {
    let node_id = row.id.clone();
    let route = route_label(ingress);
    let (tx, mut rx) = mpsc::unbounded_channel::<Out>();
    let bytes_in = Arc::new(AtomicU64::new(0));
    let bytes_out = Arc::new(AtomicU64::new(0));
    let conn_id = with_engine(|e| {
        e.next_conn += 1;
        let id = e.next_conn;
        if let Some(old) = e.conns.insert(
            node_id.clone(),
            Conn { conn_id: id, tx: tx.clone(), route, offer: None, bytes_in: bytes_in.clone(), bytes_out: bytes_out.clone() },
        ) {
            let _ = old.tx.send(Out::Close);
        }
        id
    });
    // Welcome: what we still track on this node, plus learned routes.
    let (welcome, pending_cancels) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::touch_seen(&conn, &node_id, route, hello.protocol, &hello.node_version, compute::now());
        store::record_event(&conn, "node", "attach", Some(&node_id), None, None, serde_json::json!({"route": route, "version": hello.node_version}));
        let active = store::jobs_in_states(&conn, &node_id, store::ACTIVE_STATES);
        let resume = active
            .iter()
            .map(|j| ResumeHint { job_id: j.id.clone(), generation: j.generation as u32, since_seq: logs::last_seq(&j.id, j.generation) })
            .collect();
        // Cancels still owed: a forced cancel, and (shared nodes) the "only
        // if not started" ones (V57) the node never answered.
        let cancels: Vec<Frame> = active
            .iter()
            .filter_map(|j| {
                let reason = match j.reason.as_deref() {
                    Some("cancel_requested") => "cancelled",
                    Some(pending::CANCEL) if j.node_queued => frames::CANCEL_IF_QUEUED,
                    Some(pending::WITHDRAW) | Some(pending::TIMEOUT) if j.node_queued => frames::CANCEL_WITHDRAW,
                    _ => return None,
                };
                Some(Frame::Cancel(Cancel { job_id: j.id.clone(), generation: j.generation as u32, reason: reason.into() }))
            })
            .collect();
        (
            Frame::Welcome(Welcome {
                node_id: node_id.clone(),
                name: row.name.clone(),
                controller: controller_label(),
                resume,
                routes: row.routes.clone(),
                narrow: None,
                server_time: compute::now(),
            }),
            cancels,
        )
    };
    let _ = tx.send(Out::Frame(Box::new(welcome)));
    for c in pending_cancels {
        let _ = tx.send(Out::Frame(Box::new(c)));
    }
    // Node admin: a back-online alert if an offline one went out.
    {
        let (n, b) = (node_id.clone(), hello.boot_id.clone());
        let _ = tokio::task::spawn_blocking(move || crate::compute_admin::on_attach(&n, &b)).await;
    }
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, serde_json::json!({"compute": "node_online"}));
    notify_change();

    let (mut sink, mut source) = ws.split();
    // Frames are handled in order on one blocking worker per connection.
    let (work_tx, work_rx) = std::sync::mpsc::channel::<Frame>();
    let worker_node = node_id.clone();
    let worker_route = route;
    let worker = tokio::task::spawn_blocking(move || frame_worker(worker_node, worker_route, conn_id, work_rx));

    let mut ping = tokio::time::interval(Duration::from_secs(frames::PING_SECS));
    ping.tick().await;
    let mut seq_out: u64 = 0;
    let mut seq_in: u64 = 0;
    let why: &str;
    loop {
        tokio::select! {
            _ = ping.tick() => {
                if sink.send(Message::Ping(Vec::new())).await.is_err() { why = "ping_failed"; break; }
            }
            out = rx.recv() => {
                match out {
                    Some(Out::Frame(f)) => {
                        let text = match frames::seal(&key, Direction::ControllerToNode, &session, seq_out, &f) {
                            Ok(t) => t,
                            Err(e) => { log_debug!("[compute] seal {}: {e}", f.kind()); continue; }
                        };
                        seq_out += 1;
                        bytes_out.fetch_add(text.len() as u64, Ordering::Relaxed);
                        let revoked = matches!(*f, Frame::Revoked(_));
                        if sink.send(Message::Text(text)).await.is_err() { why = "send_failed"; break; }
                        if revoked {
                            tokio::time::sleep(Duration::from_millis(200)).await;
                            why = "revoked";
                            break;
                        }
                    }
                    Some(Out::Close) | None => { why = "replaced"; break; }
                }
            }
            msg = source.next() => {
                let Some(Ok(msg)) = msg else { why = "node_closed"; break; };
                match msg {
                    Message::Text(t) => {
                        bytes_in.fetch_add(t.len() as u64, Ordering::Relaxed);
                        match frames::open(&node_spki, Direction::NodeToController, &session, seq_in, &t) {
                            Ok(f) => {
                                seq_in += 1;
                                if work_tx.send(f).is_err() { why = "worker_gone"; break; }
                            }
                            Err(e) => {
                                log_debug!("[compute] node {node_id}: {e}");
                                why = "protocol_error";
                                break;
                            }
                        }
                    }
                    Message::Close(_) => { why = "node_closed"; break; }
                    _ => {}
                }
            }
        }
    }
    let _ = sink.send(Message::Close(None)).await;
    drop(work_tx);
    let _ = worker.await;
    let removed = with_engine(|e| {
        if e.conns.get(&node_id).is_some_and(|c| c.conn_id == conn_id) {
            e.conns.remove(&node_id);
            true
        } else {
            false
        }
    });
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::add_usage(&conn, &node_id, &day(), route, bytes_in.swap(0, Ordering::Relaxed), bytes_out.swap(0, Ordering::Relaxed));
        store::touch_seen(&conn, &node_id, route, hello.protocol, &hello.node_version, compute::now());
        store::record_event(&conn, "node", "detach", Some(&node_id), None, None, serde_json::json!({"why": why}));
    }
    if removed {
        k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, serde_json::json!({"compute": "node_offline"}));
    }
    notify_change();
}

// ── frames from a node ───────────────────────────────────────────────

struct LogTally {
    seq: u64,
    bytes: u64,
    flushed: Instant,
}

fn frame_worker(node_id: String, route: &'static str, conn_id: u64, rx: std::sync::mpsc::Receiver<Frame>) {
    let mut tallies: HashMap<(String, u32), LogTally> = HashMap::new();
    let mut snapshot_seen: HashSet<String> = HashSet::new();
    while let Ok(f) = rx.recv() {
        handle_frame(&node_id, route, conn_id, f, &mut tallies, &mut snapshot_seen);
    }
    for ((job, _), t) in tallies {
        flush_tally(&job, &t);
    }
}

fn handle_frame(node_id: &str, route: &'static str, conn_id: u64, f: Frame, tallies: &mut HashMap<(String, u32), LogTally>, snapshot_seen: &mut HashSet<String>) {
    let node_id = node_id.to_string();
    match f {
        Frame::Offer(o) => on_offer(&node_id, o),
        Frame::JobsSnapshot(s) => on_snapshot(&node_id, s, snapshot_seen),
        Frame::Need(n) => on_need(&node_id, route, n),
        Frame::SecretNeed(n) => on_secret_need(&node_id, conn_id, n),
        Frame::State(s) => {
            if let Some(t) = tallies.remove(&(s.job_id.clone(), s.generation)) {
                flush_tally(&s.job_id, &t);
            }
            on_state(&node_id, s);
        }
        Frame::Log(l) => on_log(tallies, l),
        Frame::Receipt(r) => on_receipt(&node_id, r),
        Frame::Goodbye(g) => audit("node", "goodbye", Some(&node_id), serde_json::json!({"reason": g.reason})),
        // Node admin (prd-k2-node-admin-v1). A node another server owns
        // gets no node admin from here (V52): an admin frame from it is
        // only recorded, never stored as this server's health or update.
        Frame::AdminResult(_) | Frame::UpdateState(_) | Frame::Health(_) if node_is_tenant(&node_id) => {
            audit("node", "tenant_admin_frame_dropped", Some(&node_id), serde_json::json!({"kind": f.kind()}))
        }
        Frame::AdminResult(r) => crate::compute_admin::on_admin_result(&node_id, r),
        Frame::UpdateState(u) => crate::compute_admin::on_update_state(&node_id, u),
        Frame::Health(h) => crate::compute_admin::on_health(&node_id, h),
        Frame::ArtifactChunk(c) => on_artifact_chunk(&node_id, c),
        Frame::ArtifactFailed(f) => on_artifact_failed(&node_id, f),
        Frame::QueueView(v) => on_queue_view(&node_id, v),
        other => log_debug!("[compute] node {node_id} sent {}", other.kind()),
    }
}

fn node_is_tenant(node_id: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::node_by_id(&conn, node_id).is_some_and(|n| n.is_tenant())
}

// ── test seams (headless contract tests: a fake node session) ────────

/// The frames this server sends a fake node session ([`test_attach`]).
#[doc(hidden)]
pub struct TestConn {
    rx: mpsc::UnboundedReceiver<Out>,
}

impl TestConn {
    /// Every frame sent so far (drained).
    pub fn frames(&mut self) -> Vec<Frame> {
        let mut out = Vec::new();
        while let Ok(o) = self.rx.try_recv() {
            if let Out::Frame(f) = o {
                out.push(*f);
            }
        }
        out
    }
}

/// Test seam: a live session for `node_id` with `offer`, without a
/// socket. What this server sends it comes out of [`TestConn::frames`];
/// what the node "sends" goes in through [`test_frame`].
#[doc(hidden)]
pub fn test_attach(node_id: &str, offer: Offer) -> TestConn {
    let (tx, rx) = mpsc::unbounded_channel::<Out>();
    with_engine(|e| {
        e.next_conn += 1;
        let id = e.next_conn;
        e.conns.insert(
            node_id.to_string(),
            Conn { conn_id: id, tx, route: "loopback", offer: None, bytes_in: Arc::new(AtomicU64::new(0)), bytes_out: Arc::new(AtomicU64::new(0)) },
        );
    });
    let conn_id = with_engine(|e| e.conns.get(node_id).map(|c| c.conn_id)).unwrap_or(0);
    handle_frame(node_id, "loopback", conn_id, Frame::Offer(offer), &mut HashMap::new(), &mut HashSet::new());
    TestConn { rx }
}

/// Test seam: the fake node session is gone (offline).
#[doc(hidden)]
pub fn test_detach(node_id: &str) {
    with_engine(|e| e.conns.remove(node_id));
}

/// Test seam: one frame from the fake node, through the same handlers as
/// a live session (blocking).
#[doc(hidden)]
pub fn test_frame(node_id: &str, f: Frame) {
    let conn_id = with_engine(|e| e.conns.get(node_id).map(|c| c.conn_id)).unwrap_or(0);
    handle_frame(node_id, "loopback", conn_id, f, &mut HashMap::new(), &mut HashSet::new());
}

/// Test seam: wait (≤ `timeout`) until no secret release is in flight
/// (they run on their own threads, SE9).
#[doc(hidden)]
pub fn test_wait_releases(timeout: Duration) -> bool {
    let t0 = Instant::now();
    while RELEASES.load(Ordering::SeqCst) > 0 {
        if t0.elapsed() > timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

/// Secret releases running now (test seam).
static RELEASES: AtomicU64 = AtomicU64::new(0);

/// `secret_need` (SE9–SE12): on its own thread, so this connection's frame
/// loop never waits on the vault (SV6). The answer goes only to the
/// connection that asked (SV5); a node that reconnected meanwhile times
/// out and fails the attempt `secret_unavailable`.
/// Counts a release thread while it lives (test seam), panic or not.
struct ReleaseCount;

impl Drop for ReleaseCount {
    fn drop(&mut self) {
        RELEASES.fetch_sub(1, Ordering::SeqCst);
    }
}

fn on_secret_need(node_id: &str, conn_id: u64, n: frames::SecretNeed) {
    let node = node_id.to_string();
    RELEASES.fetch_add(1, Ordering::SeqCst);
    std::thread::spawn(move || {
        let _counted = ReleaseCount;
        // Only a job of the asking node takes a place (round 2, item 3: a
        // node can't burn another node's attempt). Then the caps (A-L1/L2):
        // one release in flight per (job, generation), one answer per
        // attempt; past that, dropped unanswered. The guard frees the
        // in-flight place even if the release panics.
        if !crate::compute_secrets::job_on_node(&node, &n.job_id) {
            return;
        }
        let Some(_place) = crate::compute_secrets::need_begin(&n) else { return };
        let r = crate::compute_secrets::release(&node, &n);
        let answer = match r {
            Ok((env, names, redactor)) => {
                crate::compute_secrets::belt_arm(&n.job_id, n.generation, redactor);
                Some((Frame::SecretEnv(env), Ok(names)))
            }
            Err(Some((code, message))) => Some((
                Frame::SecretRefused(frames::SecretRefused { job_id: n.job_id.clone(), generation: n.generation, code: code.to_string(), message: message.clone() }),
                Err((code, message)),
            )),
            // Not a job of this node: dropped, no answer and no k2.db row.
            Err(None) => None,
        };
        if let Some((frame, recorded)) = answer {
            crate::compute_secrets::record(&node, &n, &recorded);
            if !send_to_conn(&node, conn_id, frame) {
                log_debug!("[compute] secret answer for {} dropped: node {node} reconnected or left", n.job_id);
                if recorded.is_ok() {
                    crate::compute_secrets::belt_disarm(&n.job_id, n.generation);
                }
            }
            notify_change();
        }
    });
}

/// Queue a frame on one connection only (same `conn_id`), never on a newer
/// one of the same node.
fn send_to_conn(node_id: &str, conn_id: u64, f: Frame) -> bool {
    let tx = with_engine(|e| e.conns.get(node_id).filter(|c| c.conn_id == conn_id).map(|c| c.tx.clone()));
    tx.is_some_and(|tx| tx.send(Out::Frame(Box::new(f))).is_ok())
}

fn flush_tally(job: &str, t: &LogTally) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::set_job_log(&conn, job, t.seq, t.bytes);
}

fn on_offer(node_id: &str, o: Offer) {
    let (before, tenant, after) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let before = store::node_by_id(&conn, node_id);
        // V52 first, from what this server already knew (review A-H1): a
        // node another server owns stays one whatever its offer says.
        let tenant = share::is_tenant(&o) || before.as_ref().is_some_and(store::NodeRow::is_tenant);
        store::store_offer(&conn, node_id, &o, compute::now());
        // The node is the truth for its share, but never for its role
        // upward: `store_share` keeps a tenant a tenant.
        store::store_share(&conn, node_id, o.share.as_ref(), compute::now());
        (before, tenant, store::node_by_id(&conn, node_id))
    };
    if let (Some(b), Some(a)) = (&before, &after) {
        // MT35: owned → tenant (an owner change on the node gave it to
        // another server). The only flip an offer can make.
        if b.role != a.role {
            crate::compute_admin::on_role_change(node_id, &b.role, a);
        }
        // MT36: the node says this server owns it now. Never taken at its
        // word: the person here is asked to take it over (in person).
        if a.owner_claim() && !b.owner_claim() {
            crate::compute_admin::on_owner_claim(a);
        }
    }
    // V52: a node another server owns never gets node admin from here
    // (no approval-key pin, no queued admin commands).
    let admin = !tenant && o.features.iter().any(|f| f == k2_core::compute::proto::admin::FEATURE_ADMIN);
    let offer_copy = admin.then(|| o.clone());
    let share_now = o.share.clone();
    with_engine(|e| {
        if let Some(c) = e.conns.get_mut(node_id) {
            c.offer = Some(o);
        }
    });
    // Node admin: pin the approval key, send approved commands.
    if let Some(o) = offer_copy {
        crate::compute_admin::on_offer(node_id, &o);
    }
    if let Some(b) = before {
        crate::compute_place::share_changed(&b, share_now.as_ref());
    }
    kick(node_id);
    notify_change();
}

// ── the node's queue view (`k2 compute queue`) ───────────────────────

/// `queue_get` answers being waited for, by request id.
static QUEUE_WAITERS: (Mutex<Option<HashMap<String, Option<QueueView>>>>, Condvar) = (Mutex::new(None), Condvar::new());

fn on_queue_view(node_id: &str, v: QueueView) {
    let (m, cv) = &QUEUE_WAITERS;
    let mut g = m.lock().unwrap_or_else(|p| p.into_inner());
    match g.as_mut().and_then(|w| w.get_mut(&v.request_id)) {
        Some(slot) => {
            *slot = Some(v);
            cv.notify_all();
        }
        None => log_debug!("[compute] node {node_id}: queue_view nobody asked for"),
    }
}

/// Ask a node that offers `queue` for its queue as this server may see
/// it, and wait up to `timeout` for the answer.
pub fn queue_get(node_id: &str, timeout: Duration) -> Result<QueueView, (&'static str, String)> {
    let request_id = crypto::random_id().map_err(|e| ("internal", e))?;
    let (m, cv) = &QUEUE_WAITERS;
    m.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashMap::new).insert(request_id.clone(), None);
    let sent = send_to(node_id, Frame::QueueGet(pshare::QueueGet { request_id: request_id.clone() }));
    let deadline = Instant::now() + timeout;
    let mut g = m.lock().unwrap_or_else(|p| p.into_inner());
    loop {
        if let Some(Some(v)) = g.as_ref().and_then(|w| w.get(&request_id)) {
            let v = v.clone();
            g.as_mut().map(|w| w.remove(&request_id));
            return Ok(v);
        }
        let now = Instant::now();
        if !sent || now >= deadline {
            g.as_mut().map(|w| w.remove(&request_id));
            return Err(if sent {
                ("queue_timeout", "the node didn't answer with its queue in time; try again".to_string())
            } else {
                (compute::codes::NODE_OFFLINE, "the node is offline".to_string())
            });
        }
        g = cv.wait_timeout(g, deadline - now).map(|(g, _)| g).unwrap_or_else(|p| p.into_inner().0);
    }
}

fn on_log(tallies: &mut HashMap<(String, u32), LogTally>, l: LogChunk) {
    let Ok(data) = crypto::unb64(&l.data) else { return };
    if data.len() > frames::MAX_LOG_CHUNK {
        return;
    }
    // The belt (secret-env SE24): a released attempt's chunks are
    // redacted here too, per chunk, before anything is stored.
    let data = crate::compute_secrets::belt(&l.job_id, l.generation, &data).unwrap_or(data);
    // The generation is checked against the job row lazily (a stale
    // attempt's chunks land in that attempt's own file).
    match logs::append(&l.job_id, l.generation as i64, l.seq, l.stream, &data) {
        Ok(true) => {
            let t = tallies.entry((l.job_id.clone(), l.generation)).or_insert(LogTally { seq: 0, bytes: 0, flushed: Instant::now() });
            t.seq = l.seq;
            t.bytes += data.len() as u64;
            if t.flushed.elapsed() > Duration::from_secs(1) {
                t.flushed = Instant::now();
                flush_tally(&l.job_id, t);
            }
            notify_change();
        }
        Ok(false) => {}
        Err(e) => log_debug!("[compute] log append {}: {e}", l.job_id),
    }
}

/// Apply one node observation of a job (live `state` or a snapshot row).
/// `queue`: the node's position for a job in its queue (protocol 3).
#[allow(clippy::too_many_arguments)]
fn apply_observation(
    node_id: &str,
    job_id: &str,
    generation: u32,
    state: JobState,
    reason: Option<&str>,
    detail: Option<&str>,
    exit: Option<&frames::ExitInfo>,
    src: Option<&frames::SrcRan>,
    queue: Option<&pshare::QueuePos>,
) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Some(job) = store::job_by_id(&conn, job_id) else { return };
    if job.node_id != node_id || job.generation != generation as i64 {
        return; // another node, or a stale attempt
    }
    if job.is_terminal() {
        return;
    }
    let now = compute::now();
    let asked = job.reason.as_deref();
    // ── a shared node's queue (protocol 3) ──
    if state == JobState::Queued && reason == Some(pshare::codes::NODE_QUEUED) {
        // The node took the job into its queue: it stays `assigned` here
        // and every view shows it as queued, at the node's position. A
        // cancel or withdraw still owed keeps its marker until the node
        // answers it.
        let keep = asked == Some("cancel_requested") || pending::any(asked);
        let _ = store::mark_node_queued(&conn, job_id, job.generation, queue, (!keep).then_some(pshare::codes::NODE_QUEUED), now);
        return;
    }
    if state == JobState::Cancelled {
        // V46: the node gave a queued job back (`withdrawn`, or
        // `superseded` by the resume rule). That is never the job's end
        // here unless someone asked to cancel it: it goes to the move path.
        let user_cancel = asked == Some("cancel_requested") || asked == Some(pending::CANCEL);
        let given_back = matches!(reason, Some(pshare::codes::WITHDRAWN) | Some(pshare::codes::SUPERSEDED))
            || matches!(asked, Some(pending::WITHDRAW) | Some(pending::TIMEOUT));
        if given_back && !user_cancel {
            drop(conn);
            return given_back_job(node_id, &job);
        }
    }
    if state == JobState::Queued {
        // The node refused the assign. A cancel already asked for ends it
        // here (the node never started it, so it would answer that Cancel
        // with `unknown`, and re-queueing would run it later).
        if asked == Some("cancel_requested") || asked == Some(pending::CANCEL) {
            // `--supersede`: a newer job replaced it (prd-compute-job-tiers-v1).
            let (end, end_detail) = replaced_end(&job, asked).unwrap_or(("cancelled", None));
            let _ = store::set_job_state(&conn, job_id, JobState::Cancelled, Some(end), end_detail.as_deref(), now);
            store::record_event(
                &conn,
                "node",
                "job_end",
                Some(node_id),
                Some(&job.workspace_id),
                Some(job_id),
                serde_json::json!({"state": "cancelled", "reason": end, "refused": reason}),
            );
            drop(conn);
            finalize(job_id);
            return;
        }
        // Otherwise it waits for a fresh offer (a refusal is never in the
        // node's ledger, so the same attempt may be sent again).
        let _ = store::set_job_state(&conn, job_id, JobState::Queued, reason, detail, now);
        store::clear_node_queued(&conn, job_id);
        return;
    }
    if job.node_queued {
        // Started (or ended): it left the node's queue.
        store::clear_node_queued(&conn, job_id);
    }
    // Keep a pending cancel visible until the node reports the end.
    let keep_cancel = asked == Some("cancel_requested") && !state.is_terminal();
    // V57: a cancel without --force (or a withdraw) lost the race with the
    // start: the node left the job running and said so.
    let lost_race = asked == Some(pending::CANCEL) && !state.is_terminal();
    if lost_race {
        store::record_event(&conn, "node", "job_cancel_lost", Some(node_id), Some(&job.workspace_id), Some(job_id), serde_json::json!({"state": state.as_str()}));
    }
    let reason = if keep_cancel {
        Some("cancel_requested")
    } else if lost_race {
        Some(pshare::codes::JOB_STARTED)
    } else {
        reason
    };
    // A shared node ends a queued job by itself when this server's share
    // ends or is revoked: a result (never retried), said plainly.
    let ended_by_node = match (state, reason) {
        (JobState::Cancelled, Some(pshare::codes::SHARE_EXPIRED)) => Some("this server's share on the node has ended (the node's owner can extend it)"),
        (JobState::Cancelled, Some(pshare::codes::SHARE_REVOKED)) => Some("the node's owner removed this server from the node"),
        _ => None,
    };
    let detail = detail.or(ended_by_node);
    // `--supersede`: the node confirmed our "only if not started" cancel of
    // a job a newer one replaced: it ends `replaced`, not `cancelled`.
    let replaced = if state == JobState::Cancelled { replaced_end(&job, asked) } else { None };
    let replaced_detail = replaced.as_ref().and_then(|(_, d)| d.clone());
    let (reason, detail) = match &replaced {
        Some((r, _)) => (Some(*r), replaced_detail.as_deref()),
        None => (reason, detail),
    };
    // A job someone asked to cancel that comes back interrupted (the node
    // restarted, rebooted or lost it) or unknown ends `cancelled`: that is
    // what was asked, and it is never sent again (k2n4, 2026-10-21: two
    // cancelled jobs whose ends a full disk lost came back
    // `interrupted/node_restart` and were re-dispatched).
    let cancel_owed = asked == Some("cancel_requested");
    let interrupted_detail;
    let (state, reason, detail) = if cancel_owed && matches!(state, JobState::Interrupted | JobState::Unknown) {
        interrupted_detail = format!("cancelled as asked; the node then reported {}{}", state.as_str(), reason.map(|r| format!("/{r}")).unwrap_or_default());
        (JobState::Cancelled, Some("cancelled"), Some(interrupted_detail.as_str()))
    } else {
        (state, reason, detail)
    };
    let _ = store::set_job_state(&conn, job_id, state, reason, detail, now);
    if let Some(s) = src {
        store::set_job_src(&conn, job_id, &s.commit, &s.tree);
    }
    if let Some(x) = exit {
        store::set_job_exit(&conn, job_id, x.code, x.signal);
    }
    if !state.is_terminal() {
        return;
    }
    store::set_job_log(&conn, job_id, logs::last_seq(job_id, job.generation), 0);
    store::record_event(
        &conn,
        "node",
        "job_end",
        Some(node_id),
        Some(&job.workspace_id),
        Some(job_id),
        serde_json::json!({"state": state.as_str(), "reason": reason, "exit": exit.and_then(|x| x.code)}),
    );
    drop(conn);
    // Infrastructure failures retry (with backoff, on another node for
    // `--any`); results never do. A retried job gets no message yet. A job
    // whose cancel was asked for (slice A) is never retried.
    if !cancel_owed && crate::compute_place::after_attempt(job_id) {
        return;
    }
    finalize(job_id);
}

/// A shared node gave a queued job back (`cancelled/withdrawn`, or
/// `superseded`): it never started there. V46: requeue it under a NEW
/// generation, with no retry used, no failed attempt and no health
/// strike, then move it (`--any`) or send it again. A withdraw for
/// `--queue-timeout` fails it instead.
fn given_back_job(node_id: &str, job: &store::JobRow) {
    let now = compute::now();
    if job.reason.as_deref() == Some(pending::TIMEOUT) {
        let limit = job.ctl.queue_timeout_secs.unwrap_or(0);
        let detail = format!(
            "not started within {} (it waited in the node's queue{})",
            compute::message::duration(limit),
            job.node_blocked_by.as_deref().map(|b| format!(": {}", share::blocked_words(b))).unwrap_or_default()
        );
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = store::set_job_state(&conn, &job.id, JobState::Failed, Some("queue_timeout"), Some(&detail), now);
            store::clear_node_queued(&conn, &job.id);
            store::record_event(&conn, "controller", "job_queue_timeout", Some(node_id), Some(&job.workspace_id), Some(&job.id), serde_json::json!({"nodeQueued": true}));
        }
        finalize(&job.id);
        notify_change();
        return;
    }
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if store::requeue_new_generation(&conn, &job.id, job.generation, "withdrawn") != Ok(true) {
            return;
        }
        store::record_event(
            &conn,
            "controller",
            "withdraw",
            Some(node_id),
            Some(&job.workspace_id),
            Some(&job.id),
            serde_json::json!({"generation": job.generation, "newGeneration": job.generation + 1, "asked": job.reason}),
        );
        store::job_by_id(&conn, &job.id)
    };
    if let Some(r) = row.filter(|r| r.ctl.any.is_some()) {
        if crate::compute_place::try_move(&r, &[], false, "withdrawn").is_some() {
            return;
        }
    }
    kick(node_id);
    notify_change();
}

/// A job reached its end: message the requesting session once (detached).
pub fn finalize(job_id: &str) {
    // A shard reports to its split run, never to the session: the run
    // sends one message when its last shard ends.
    let parent = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::job_by_id(&conn, job_id).and_then(|j| j.ctl.shard.clone().filter(|_| j.is_terminal()).map(|s| s.parent))
    };
    if let Some(p) = parent {
        crate::compute_shards::child_ended(&p);
        return;
    }
    let (job, node_name, ws_path) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Some(job) = store::job_by_id(&conn, job_id) else { return };
        if job.ctl.shard.is_some() {
            return;
        }
        // Detached jobs always get a message. An attached job gets one too
        // when nobody is following its logs any more (the agent's shell
        // tool timed out and killed `k2 compute run`): otherwise its result
        // would only ever be found by polling.
        let wants = job.detach || (job.session_id.is_some() && !follower_recent(&job.id));
        // A job its requester cancelled: they know how it ended.
        let wants = wants && !(job.state == "cancelled" && job.ctl.cancelled_by_requester);
        if !job.is_terminal() || !wants || job.notified_at.is_some() {
            return;
        }
        store::mark_notified(&conn, job_id, compute::now());
        let node_name = store::node_by_id(&conn, &job.node_id).map(|n| n.name).unwrap_or_else(|| job.node_id.clone());
        let ws_path = k2_core::db::schema::Project::get(&conn, &job.workspace_id).ok().map(|p| p.path);
        (job, node_name, ws_path)
    };
    let secs = match (job.started_at.or(job.assigned_at), job.ended_at) {
        (Some(a), Some(b)) => Some(b - a),
        _ => None,
    };
    let tests = compute::result::outputs(&job).and_then(|o| o.tests);
    let text = if job.ctl.shards.is_empty() {
        compute::message::completion_text(&job.id, &node_name, &job.state, job.exit_code, job.signal, job.reason.as_deref(), secs, tests.as_ref())
    } else {
        compute::message::shards_completion_text(
            &job.id,
            job.ctl.shards.len(),
            &job.state,
            job.exit_code,
            job.signal,
            job.reason.as_deref(),
            secs,
            tests.as_ref(),
        )
    };
    let session = job.session_id.clone();
    let id = job.id.clone();
    // In the daemon's runtime: a wake spawns the agent's session (`crate::rt`).
    let spawned = crate::rt::spawn("compute-done", move || deliver_completion(&id, session.as_deref(), ws_path.as_deref(), &text));
    if let Err(e) = spawned {
        k2_core::log_debug!("[compute] job {} completion not delivered: {e}", job.id);
    }
}

/// Live message to the requesting session, else the workspace inbox.
fn deliver_completion(job_id: &str, session: Option<&str>, ws_path: Option<&str>, text: &str) {
    if let Some(sid) = session.filter(|s| !s.is_empty()) {
        if test_sink(sid, text) {
            return;
        }
        let r = crate::workspace_msg::deliver_live(sid, text, "compute", "", true, crate::workspace_msg::DEFAULT_WAKE_TIMEOUT);
        if r.success {
            return;
        }
    }
    if let Some(p) = ws_path {
        let title = format!("compute job {} ended", compute::message::short(job_id));
        if let Err(e) = k2_core::inbox::compose(std::path::Path::new(p), &title, text, None, Some("compute"), Some("compute")) {
            log_debug!("[compute] inbox fallback for {job_id}: {e}");
        }
    }
}

/// Test seam: a session id registered here receives completion texts
/// instead of a real PTY (headless contract tests).
static TEST_SINK: Mutex<Option<HashMap<String, Vec<String>>>> = Mutex::new(None);

// Used by the lib's integration tests, not the binary.
#[allow(dead_code)]
pub fn test_sink_register(session: &str) {
    TEST_SINK.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashMap::new).insert(session.to_string(), Vec::new());
}

#[allow(dead_code)]
pub fn test_sink_take(session: &str) -> Vec<String> {
    TEST_SINK
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_mut()
        .and_then(|m| m.get_mut(session).map(std::mem::take))
        .unwrap_or_default()
}

#[allow(dead_code)]
pub fn test_sink_peek(session: &str) -> Vec<String> {
    TEST_SINK
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .and_then(|m| m.get(session).cloned())
        .unwrap_or_default()
}

fn test_sink(session: &str, text: &str) -> bool {
    let mut g = TEST_SINK.lock().unwrap_or_else(|p| p.into_inner());
    match g.as_mut().and_then(|m| m.get_mut(session)) {
        Some(v) => {
            v.push(text.to_string());
            true
        }
        None => false,
    }
}

/// Keep a terminal attempt's outputs (artifacts + tests). Only for this
/// node and the attempt's own generation; capped so a node can't flood
/// the row.
fn store_outputs(node_id: &str, job_id: &str, generation: u32, o: Outputs) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Some(job) = store::job_by_id(&conn, job_id) else { return };
    if job.node_id != node_id || job.generation != generation as i64 {
        return;
    }
    if let Ok(text) = serde_json::to_string(&o.capped()) {
        store::set_job_outputs(&conn, job_id, generation as i64, &text);
    }
}

fn on_state(node_id: &str, s: StateUpdate) {
    if let Some(o) = s.outputs.clone().filter(|_| s.state.is_terminal()) {
        store_outputs(node_id, &s.job_id, s.generation, o);
    }
    apply_observation(node_id, &s.job_id, s.generation, s.state, s.reason.as_deref(), s.detail.as_deref(), s.exit.as_ref(), s.src.as_ref(), s.queue.as_ref());
    if s.state.is_terminal() {
        crate::compute_secrets::belt_disarm(&s.job_id, s.generation);
        kick(node_id);
    }
    notify_change();
}

fn on_snapshot(node_id: &str, s: JobsSnapshot, seen: &mut HashSet<String>) {
    for j in &s.jobs {
        seen.insert(format!("{}#{}", j.job_id, j.generation));
        stop_stale(node_id, j);
        apply_observation(node_id, &j.job_id, j.generation, j.state, j.reason.as_deref(), None, j.exit.as_ref(), None, None);
    }
    if !s.last {
        return;
    }
    // Jobs we think are on the node that its ledger never saw: the assign
    // was lost. Never re-sent blindly (§11.5): they become `unknown`.
    let mut lost = Vec::new();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for j in store::jobs_in_states(&conn, node_id, store::ACTIVE_STATES) {
            if !seen.contains(&format!("{}#{}", j.id, j.generation)) && j.node_queued {
                // A queued row the node no longer has never started: a new
                // generation, no retry (V46, V56).
                if store::requeue_new_generation(&conn, &j.id, j.generation, "not_on_node") == Ok(true) {
                    store::record_event(&conn, "controller", "job_requeue", Some(node_id), Some(&j.workspace_id), Some(&j.id), serde_json::json!({"why": "not_on_node", "nodeQueued": true}));
                }
                continue;
            }
            if !seen.contains(&format!("{}#{}", j.id, j.generation)) {
                let _ = store::set_job_state(&conn, &j.id, JobState::Unknown, Some("not_on_node"), None, compute::now());
                store::record_event(&conn, "controller", "job_unknown", Some(node_id), Some(&j.workspace_id), Some(&j.id), serde_json::json!({}));
                lost.push(j.id);
            }
        }
    }
    seen.clear();
    for id in lost {
        if !crate::compute_place::after_attempt(&id) {
            finalize(&id);
        }
    }
    kick(node_id);
    notify_change();
}

/// Ask a shared node to give a queued job back, or (no `--force`) to
/// cancel it only if it hasn't started (V57). The answer comes as a state.
pub fn send_cancel_if_not_started(node_id: &str, job_id: &str, generation: i64, reason: &'static str) -> bool {
    debug_assert!(frames::cancel_only_if_not_started(reason));
    send_to(node_id, Frame::Cancel(Cancel { job_id: job_id.into(), generation: generation as u32, reason: reason.into() }))
}

/// A node reconnected still running an attempt this controller already
/// ended (it was `node_lost`, or a newer attempt replaced it): tell it to
/// stop, so a lost node never keeps a slot busy for nothing.
fn stop_stale(node_id: &str, j: &frames::LedgerJob) {
    if j.state.is_terminal() {
        return;
    }
    let stale = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::job_by_id(&conn, &j.job_id).is_some_and(|row| {
            (row.generation as u32) > j.generation || (row.node_id != node_id) || (row.generation as u32 == j.generation && row.is_terminal())
        })
    };
    if stale {
        send_to(node_id, Frame::Cancel(Cancel { job_id: j.job_id.clone(), generation: j.generation, reason: "superseded".into() }));
    }
}

fn on_receipt(node_id: &str, r: ReceiptFrame) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Some(job) = store::job_by_id(&conn, &r.job_id) else { return };
    if job.node_id != node_id || job.generation != r.generation as i64 {
        return;
    }
    let Some(node) = store::node_by_id(&conn, node_id) else { return };
    let ok = crypto::pem_decode("PUBLIC KEY", &node.public_key_pem)
        .ok()
        .and_then(|spki| r.receipt.verify(&spki))
        .is_some_and(|rc| rc.job_id == job.id && rc.generation == r.generation && rc.plan_digest == job.plan_digest);
    let body = serde_json::to_string(&r.receipt).unwrap_or_default();
    store::set_job_receipt(&conn, &job.id, &body, ok);
    // The receipt lists the kept files too: if the terminal state with
    // the outputs was missed (a reconnect snapshot ended the job first),
    // the signed list fills in.
    if ok && job.summary_json.is_none() {
        let arts = crypto::pem_decode("PUBLIC KEY", &node.public_key_pem)
            .ok()
            .and_then(|spki| r.receipt.verify(&spki))
            .map(|rc| rc.artifacts)
            .unwrap_or_default();
        if !arts.is_empty() {
            if let Ok(text) = serde_json::to_string(&Outputs { artifacts: arts, ..Default::default() }.capped()) {
                store::set_job_outputs(&conn, &job.id, job.generation, &text);
            }
        }
    }
    drop(conn);
    notify_change();
}

// ── artifacts out (§11.4) ────────────────────────────────────────────

/// One artifact being pulled from a node into the controller's cache.
struct Pull {
    node_id: String,
    tmp: std::path::PathBuf,
    dst: std::path::PathBuf,
    file: Option<std::fs::File>,
    hasher: Option<crypto::Sha256Stream>,
    expect_sha: String,
    expect_bytes: u64,
    got: u64,
    next_seq: u32,
    done: Option<Result<(), (String, String)>>,
}

static PULLS: Mutex<Option<HashMap<String, Pull>>> = Mutex::new(None);

fn with_pulls<T>(f: impl FnOnce(&mut HashMap<String, Pull>) -> T) -> T {
    let mut g = PULLS.lock().unwrap_or_else(|p| p.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

/// `~/.k2/compute/artifacts/<job>-g<gen>/<sha256>`: fetched files, by hash.
pub fn artifact_cache_path(job_id: &str, generation: i64, sha256: &str) -> std::path::PathBuf {
    compute::compute_dir().join("artifacts").join(format!("{job_id}-g{generation}")).join(sha256)
}

/// The largest artifact this server pulls from a node on `route`.
/// Over the relay the default stays [`results::RELAY_MAX_FILE_BYTES`]
/// (it stops an accidental big pull through the Connect edge, PRD §12.4);
/// `over_relay` is the caller saying "yes, through the relay": then the
/// node's own collection cap ([`results::MAX_FILE_BYTES`]) is the limit,
/// as on the LAN. Release builds need it: the Linux AppImage is over
/// 100 MB and the release nodes reach z3mbpZ only through the relay.
pub fn artifact_pull_cap(route: Option<&str>, over_relay: bool) -> u64 {
    if route == Some("relay") && !over_relay {
        results::RELAY_MAX_FILE_BYTES
    } else {
        results::MAX_FILE_BYTES
    }
}

/// Why a file of `bytes` can't be pulled on `route`, if it can't.
pub fn pull_cap_refusal(route: Option<&str>, over_relay: bool, bytes: u64) -> Option<(String, String)> {
    let cap = artifact_pull_cap(route, over_relay);
    if bytes <= cap {
        return None;
    }
    let mb = bytes >> 20;
    Some(if route == Some("relay") && !over_relay {
        (
            "relay_cap".into(),
            format!(
                "{mb} MB is over the relay cap of {} MB; fetch it again with --over-relay (up to {} MB through the relay), or over the LAN or a tailnet route",
                cap >> 20,
                results::MAX_FILE_BYTES >> 20
            ),
        )
    } else {
        ("relay_cap".into(), format!("{mb} MB is over the artifact cap of {} MB", cap >> 20))
    })
}

/// Cache files already hashed once: `(bytes, mtime)` as seen then. A
/// fetch reads a file in 4 MB slices, and re-hashing a 500 MB file for
/// every slice would cost minutes; the file only changes through
/// [`fetch_artifact`]'s rename, which records it again.
static VERIFIED: Mutex<Option<HashMap<std::path::PathBuf, (u64, std::time::SystemTime)>>> = Mutex::new(None);

fn cache_stamp(p: &std::path::Path) -> Option<(u64, std::time::SystemTime)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.len(), m.modified().ok()?))
}

fn note_verified(p: &std::path::Path) {
    if let Some(st) = cache_stamp(p) {
        let mut g = VERIFIED.lock().unwrap_or_else(|e| e.into_inner());
        let m = g.get_or_insert_with(HashMap::new);
        if m.len() > 4096 {
            m.clear();
        }
        m.insert(p.to_path_buf(), st);
    }
}

/// The cache already holds exactly `info` (hashed now, or hashed before
/// and unchanged since).
fn cache_holds(dst: &std::path::Path, info: &ArtifactInfo) -> bool {
    let st = cache_stamp(dst);
    if st.is_some_and(|s| s.0 == info.bytes)
        && VERIFIED.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(dst)).copied() == st
    {
        return true;
    }
    let ok = compute::sync::file_sha256(dst).is_ok_and(|(sha, n)| sha == info.sha256 && n == info.bytes);
    if ok {
        note_verified(dst);
    }
    ok
}

fn finish_pull(p: &mut Pull, r: Result<(), (String, String)>) {
    p.file = None;
    if r.is_err() {
        let _ = std::fs::remove_file(&p.tmp);
    }
    p.done = Some(r);
}

fn on_artifact_chunk(node_id: &str, c: ArtifactChunk) {
    with_pulls(|m| {
        let Some(p) = m.get_mut(&c.request_id).filter(|p| p.node_id == node_id && p.done.is_none()) else { return };
        let bad = |why: &str| Err(("artifact_transfer".to_string(), why.to_string()));
        if c.seq != p.next_seq || c.sha256 != p.expect_sha || c.total_bytes != p.expect_bytes {
            return finish_pull(p, bad("chunk out of order or for another file"));
        }
        let Ok(data) = crypto::unb64(&c.data) else { return finish_pull(p, bad("chunk isn't base64")) };
        if data.len() > frames::TRANSFER_CHUNK || p.got + data.len() as u64 > p.expect_bytes {
            return finish_pull(p, bad("more bytes than the node reported"));
        }
        let wrote = p.file.as_mut().map(|f| std::io::Write::write_all(f, &data));
        if !matches!(wrote, Some(Ok(()))) {
            return finish_pull(p, Err(("local_write".into(), "can't write the controller's artifact cache".into())));
        }
        if let Some(h) = p.hasher.as_mut() {
            h.update(&data);
        }
        p.got += data.len() as u64;
        p.next_seq += 1;
        if c.last {
            let sha = p.hasher.take().map(|h| h.finish_hex()).unwrap_or_default();
            if p.got != p.expect_bytes || !crypto::ct_eq(sha.as_bytes(), p.expect_sha.as_bytes()) {
                return finish_pull(p, Err(("artifact_changed".into(), "the bytes don't match the hash the node reported".into())));
            }
            p.file = None;
            let r = std::fs::rename(&p.tmp, &p.dst).map_err(|e| ("local_write".to_string(), format!("cache: {e}")));
            if r.is_ok() {
                note_verified(&p.dst);
            }
            finish_pull(p, r);
        }
    });
    notify_change();
}

fn on_artifact_failed(node_id: &str, f: ArtifactFailed) {
    with_pulls(|m| {
        if let Some(p) = m.get_mut(&f.request_id).filter(|p| p.node_id == node_id && p.done.is_none()) {
            let code: String = f.code.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(48).collect();
            let msg: String = f.message.chars().take(300).collect();
            finish_pull(p, Err((code, msg)));
        }
    });
    notify_change();
}

/// Make sure the controller has `info` for this attempt, pulling it from
/// the node when needed (blocking, up to `timeout`). `over_relay`: see
/// [`artifact_pull_cap`]. `Err((code, hint))`.
pub fn fetch_artifact(job: &store::JobRow, info: &ArtifactInfo, over_relay: bool, timeout: Duration) -> Result<std::path::PathBuf, (String, String)> {
    let dst = artifact_cache_path(&job.id, job.generation, &info.sha256);
    if cache_holds(&dst, info) {
        return Ok(dst);
    }
    let (online, route, offer) = live_view(&job.node_id);
    if !online {
        return Err(("node_offline".into(), "the job's node is offline; artifacts stay on it for 7 days, fetch again when it's back".into()));
    }
    if !offer.is_some_and(|o| o.features.iter().any(|f| f == results::FEATURE_ARTIFACTS)) {
        return Err(("node_upgrade_required".into(), "that node's k2-node can't send artifacts; update it there (sudo k2-node setup)".into()));
    }
    if let Some(e) = pull_cap_refusal(route, over_relay, info.bytes) {
        return Err(e);
    }
    let dir = dst.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    std::fs::create_dir_all(&dir).map_err(|e| ("local_write".to_string(), format!("cache dir: {e}")))?;
    let request_id = crypto::random_id().map_err(|e| ("internal".to_string(), e))?;
    let tmp = dir.join(format!(".{request_id}.part"));
    let file = std::fs::File::create(&tmp).map_err(|e| ("local_write".to_string(), format!("cache file: {e}")))?;
    with_pulls(|m| {
        m.insert(
            request_id.clone(),
            Pull {
                node_id: job.node_id.clone(),
                tmp: tmp.clone(),
                dst: dst.clone(),
                file: Some(file),
                hasher: Some(crypto::Sha256Stream::default()),
                expect_sha: info.sha256.clone(),
                expect_bytes: info.bytes,
                got: 0,
                next_seq: 0,
                done: None,
            },
        )
    });
    let sent = send_to(
        &job.node_id,
        Frame::ArtifactGet(ArtifactGet { request_id: request_id.clone(), job_id: job.id.clone(), generation: job.generation as u32, name: info.name.clone() }),
    );
    let deadline = Instant::now() + timeout;
    let outcome = loop {
        let seen = change_counter();
        let done = with_pulls(|m| m.get(&request_id).and_then(|p| p.done.clone()));
        if let Some(d) = done {
            break d;
        }
        let now = Instant::now();
        if !sent || now >= deadline {
            break Err(("artifact_timeout".into(), "the node didn't send the file in time; try again".into()));
        }
        if !is_online(&job.node_id) {
            break Err(("node_offline".into(), "the node went offline during the transfer; fetch again when it's back".into()));
        }
        wait_change(seen, (deadline - now).min(Duration::from_secs(1)));
    };
    with_pulls(|m| m.remove(&request_id));
    let _ = std::fs::remove_file(&tmp);
    outcome.map(|_| dst)
}

/// A background pull one fetch started and later fetches poll
/// ([`fetch_artifact_wait`]). Keyed by the cache path, so two fetches of
/// one file share one transfer.
struct BgPull {
    done: Option<Result<std::path::PathBuf, (String, String)>>,
    at: Instant,
}

static BG_PULLS: Mutex<Option<HashMap<std::path::PathBuf, BgPull>>> = Mutex::new(None);

fn with_bg<T>(f: impl FnOnce(&mut HashMap<std::path::PathBuf, BgPull>) -> T) -> T {
    let mut g = BG_PULLS.lock().unwrap_or_else(|p| p.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

/// What [`fetch_artifact_wait`] has to say.
#[derive(Debug)]
pub enum Fetched {
    /// The file is in the cache, checked.
    Ready(std::path::PathBuf),
    /// Still coming from the node: `got` of the file's bytes so far.
    Pending { got: u64 },
}

/// Bytes received so far for the pull filling `dst`.
fn pull_progress(dst: &std::path::Path) -> u64 {
    with_pulls(|m| m.values().filter(|p| p.dst == dst).map(|p| p.got).max().unwrap_or(0))
}

/// Like [`fetch_artifact`], but the transfer runs on its own thread and
/// this waits at most `wait` for it: a big file over a slow relay takes
/// longer than one HTTP request should stay open (the CLI's GETs give up
/// after 60 s), so the caller asks again until it's `Ready`, or gets the
/// pull's error (once; the next ask starts a new pull).
pub fn fetch_artifact_wait(
    job: &store::JobRow,
    info: &ArtifactInfo,
    over_relay: bool,
    timeout: Duration,
    wait: Duration,
) -> Result<Fetched, (String, String)> {
    let dst = artifact_cache_path(&job.id, job.generation, &info.sha256);
    // Every 4 MB slice asks again: once the file is in (and hashed), answer
    // from the cache without a thread.
    let in_flight = with_bg(|m| m.contains_key(&dst));
    if !in_flight && cache_holds(&dst, info) {
        return Ok(Fetched::Ready(dst));
    }
    let start = with_bg(|m| {
        // A finished pull nobody came back for doesn't stay forever.
        m.retain(|_, b| b.done.is_none() || b.at.elapsed() < Duration::from_secs(3600));
        if m.contains_key(&dst) {
            return false;
        }
        m.insert(dst.clone(), BgPull { done: None, at: Instant::now() });
        true
    });
    if start {
        // Even the cache check runs there: hashing 500 MB is slow too.
        let (job, info, key) = (job.clone(), info.clone(), dst.clone());
        std::thread::spawn(move || {
            let r = fetch_artifact(&job, &info, over_relay, timeout);
            with_bg(|m| {
                if let Some(b) = m.get_mut(&key) {
                    b.done = Some(r);
                    b.at = Instant::now();
                }
            });
            notify_change();
        });
    }
    let deadline = Instant::now() + wait;
    loop {
        let seen = change_counter();
        let done = with_bg(|m| match m.get(&dst).map(|b| b.done.is_some()) {
            Some(true) => m.remove(&dst).and_then(|b| b.done),
            _ => None,
        });
        if let Some(r) = done {
            return r.map(Fetched::Ready);
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(Fetched::Pending { got: pull_progress(&dst) });
        }
        wait_change(seen, (deadline - now).min(Duration::from_millis(500)));
    }
}

/// Drop fetched artifacts older than the node keeps them.
fn sweep_artifact_cache() {
    let root = compute::compute_dir().join("artifacts");
    let Ok(rd) = std::fs::read_dir(&root) else { return };
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|d| d.as_secs() as i64 > results::KEEP_SECS);
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

// ── code in ──────────────────────────────────────────────────────────

/// Where a job's dirty blob is kept between submit and transfer.
pub fn dirty_blob_path(job_id: &str) -> std::path::PathBuf {
    compute::compute_dir().join("blobs").join(format!("{job_id}.dirty.tar"))
}

fn on_need(node_id: &str, route: &'static str, n: Need) {
    let node = node_id.to_string();
    std::thread::spawn(move || {
        let fail = |code: &str, msg: String| {
            send_to(&node, Frame::TransferFailed(TransferFailed { job_id: n.job_id.clone(), generation: n.generation, kind: n.kind, code: code.into(), message: msg }));
        };
        let (job, ws_path) = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let job = store::job_by_id(&conn, &n.job_id);
            let path = job
                .as_ref()
                .and_then(|j| k2_core::db::schema::Project::get(&conn, &j.workspace_id).ok().map(|p| p.path));
            (job, path)
        };
        let Some(job) = job.filter(|j| j.node_id == node && j.generation == n.generation as i64) else {
            return fail("job_not_found", "no such job attempt on this node".into());
        };
        let Some(src) = job.plan().and_then(|p| p.src) else {
            return fail("no_source", "this job has no source".into());
        };
        let blob = match n.kind {
            TransferKind::Bundle => {
                if n.want != src.commit {
                    return fail("bad_need", "the node asked for another commit".into());
                }
                let Some(ws) = ws_path else { return fail("workspace_gone", "the workspace is gone".into()) };
                let root = match compute::sync::repo_root(std::path::Path::new(&ws)) {
                    Ok(r) => r,
                    Err(e) => return fail("sync_failed", e),
                };
                let dir = compute::compute_dir().join("blobs");
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    return fail("sync_failed", format!("blob dir: {e}"));
                }
                let out = dir.join(format!("{}-g{}.bundle", job.id, job.generation));
                match compute::sync::create_bundle(&root, &src.commit, &n.have, &job.id, &out) {
                    Ok(_) => (out, true),
                    Err(e) => return fail("sync_failed", e),
                }
            }
            TransferKind::Dirty => (dirty_blob_path(&job.id), false),
        };
        let (path, delete_after) = blob;
        let (sha, bytes) = match compute::sync::file_sha256(&path) {
            Ok(v) => v,
            Err(e) => return fail("sync_failed", e),
        };
        if route == "relay" && bytes > compute::RELAY_MAX_BUNDLE_BYTES {
            if delete_after {
                let _ = std::fs::remove_file(&path);
            }
            return fail(
                "relay_cap",
                format!("{} MB is over the relay cap of {} MB; use the LAN, a tailnet route, or give the node fetch access to the remote", bytes >> 20, compute::RELAY_MAX_BUNDLE_BYTES >> 20),
            );
        }
        if let Some(d) = src.dirty.as_ref().filter(|_| n.kind == TransferKind::Dirty) {
            if d.sha256 != sha {
                return fail("sync_failed", "the dirty blob changed since submit".into());
            }
        }
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(e) => return fail("sync_failed", format!("read blob: {e}")),
        };
        if delete_after {
            let _ = std::fs::remove_file(&path);
        }
        let chunks: Vec<&[u8]> = if data.is_empty() { vec![&[][..]] } else { data.chunks(frames::TRANSFER_CHUNK).collect() };
        let last = chunks.len().saturating_sub(1);
        for (i, c) in chunks.into_iter().enumerate() {
            send_to(
                &node,
                Frame::Transfer(Transfer {
                    job_id: job.id.clone(),
                    generation: n.generation,
                    kind: n.kind,
                    seq: i as u32,
                    data: crypto::b64(c),
                    last: i == last,
                    total_bytes: bytes,
                    sha256: sha.clone(),
                }),
            );
        }
    });
}

/// Queue a frame on a node's live session (node admin). False when the
/// node isn't connected.
pub fn send_frame(node_id: &str, f: Frame) -> bool {
    send_to(node_id, f)
}

fn send_to(node_id: &str, f: Frame) -> bool {
    let tx = with_engine(|e| e.conns.get(node_id).map(|c| c.tx.clone()));
    tx.is_some_and(|tx| tx.send(Out::Frame(Box::new(f))).is_ok())
}

// ── placement ────────────────────────────────────────────────────────

/// Queued jobs on a node as the scheduler sees them.
pub fn queued_view(conn: &rusqlite::Connection, node_id: &str) -> (Vec<scheduler::QueuedJob>, scheduler::Load) {
    let now = compute::now();
    let queue = store::jobs_in_states(conn, node_id, &["queued"])
        .into_iter()
        // A retry waiting out its backoff isn't placeable yet.
        .filter(|j| j.ctl.not_before.is_none_or(|nb| nb <= now))
        .map(|j| {
            let plan = j.plan();
            scheduler::QueuedJob {
                id: j.id.clone(),
                workspace_id: j.workspace_id.clone(),
                created_at: j.created_at,
                exclusive: j.exclusive,
                cpu_millis: plan.as_ref().and_then(|p| p.limits.cpu_millis),
                mem_bytes: plan.as_ref().and_then(|p| p.limits.mem_bytes),
                disk_bytes: 0,
                needs_git: plan.as_ref().is_some_and(|p| p.src.is_some()),
                tier: j.tier(),
            }
        })
        .collect();
    let mut load = scheduler::Load::default();
    for j in store::jobs_in_states(conn, node_id, store::ACTIVE_STATES) {
        *load.running_by_workspace.entry(j.workspace_id.clone()).or_insert(0) += 1;
        load.exclusive_running |= j.exclusive;
    }
    (queue, load)
}

/// One placement pass on a SHARED node (protocol 3, §6): every job this
/// server has queued for it goes into the node's queue at once, in this
/// server's own order (CI, then Priority, then workspace rotation, FIFO),
/// as long as its share has room. Nothing here waits for free capacity:
/// the node decides who runs.
struct SharedPass {
    /// Send now, in this order.
    send: Vec<String>,
    /// Never startable under this share or node: fail (a request error).
    fail: Vec<(String, frames::Refusal)>,
    /// Waiting on this server, with a position and why.
    waiting: BTreeMap<String, (usize, frames::Refusal)>,
}

/// The send order into a shared node (the third orderer, vs-live T2): CI,
/// then Priority, each by `created_at`, then everything else in workspace
/// rotation (after the last-served workspace), FIFO within one. Without
/// the tier first a CI job would wait on this server behind a full share
/// (`tenant_queue_full`) and never reach the node's queue to jump it.
fn rotation_order(mut jobs: Vec<store::JobRow>, last_ws: Option<&str>) -> Vec<store::JobRow> {
    jobs.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.id.cmp(&b.id)));
    let (urgent, jobs): (Vec<store::JobRow>, Vec<store::JobRow>) = jobs.into_iter().partition(|j| j.tier() != Tier::Normal);
    let mut urgent = urgent;
    // Stable: CI before Priority, each still oldest first.
    urgent.sort_by_key(|j| std::cmp::Reverse(j.tier()));
    let mut order: Vec<String> = jobs.iter().map(|j| j.workspace_id.clone()).collect();
    order.sort_unstable();
    order.dedup();
    if let Some(last) = last_ws {
        let start = order.iter().position(|w| w.as_str() > last).unwrap_or(0);
        order.rotate_left(start);
    }
    let mut by_ws: HashMap<String, std::collections::VecDeque<store::JobRow>> = HashMap::new();
    for j in jobs {
        by_ws.entry(j.workspace_id.clone()).or_default().push_back(j);
    }
    let mut out = urgent;
    loop {
        let mut any = false;
        for w in &order {
            if let Some(j) = by_ws.get_mut(w).and_then(|q| q.pop_front()) {
                out.push(j);
                any = true;
            }
        }
        if !any {
            return out;
        }
    }
}

fn shared_pass(conn: &rusqlite::Connection, node_id: &str, offer: &Offer, view: &pshare::ShareView, last_ws: Option<&str>, pause: Option<&str>) -> SharedPass {
    let now = compute::now();
    let t = compute::retry::tuning();
    let queued: Vec<store::JobRow> = store::jobs_in_states(conn, node_id, &["queued"])
        .into_iter()
        .filter(|j| j.ctl.not_before.is_none_or(|nb| nb <= now))
        .collect();
    let mut pass = SharedPass { send: Vec::new(), fail: Vec::new(), waiting: BTreeMap::new() };
    let ordered = rotation_order(queued, last_ws);
    let blocked = if let Some(p) = pause {
        Some(frames::Refusal::new(scheduler::CONTROLLER_PAUSED, format!("the server's owner {p} this node")))
    } else {
        match view.state {
            pshare::ServerStatus::Active => None,
            pshare::ServerStatus::Paused => Some(frames::Refusal::new(pshare::codes::SHARE_PAUSED, "the node's owner paused this server's share")),
            pshare::ServerStatus::Expired => Some(frames::Refusal::new(pshare::codes::SHARE_EXPIRED, "this server's share on the node has ended")),
            pshare::ServerStatus::Revoked => Some(frames::Refusal::new(pshare::codes::SHARE_REVOKED, "the node's owner revoked this server")),
            s => Some(frames::Refusal::new(pshare::codes::SERVER_NOT_APPROVED, format!("this server is {} on the node", s.as_str()))),
        }
    };
    let on_node = store::jobs_in_states(conn, node_id, store::ACTIVE_STATES).len() as u32;
    let mut room = share::room(view, on_node);
    for (i, j) in ordered.iter().enumerate() {
        let pos = i + 1;
        let wait = |why: frames::Refusal, pass: &mut SharedPass| {
            pass.waiting.insert(j.id.clone(), (pos, why));
        };
        let Some(plan) = j.plan() else { continue };
        let probe = scheduler::QueuedJob {
            id: j.id.clone(),
            workspace_id: j.workspace_id.clone(),
            created_at: j.created_at,
            exclusive: j.exclusive,
            cpu_millis: plan.limits.cpu_millis,
            mem_bytes: plan.limits.mem_bytes,
            disk_bytes: 0,
            needs_git: plan.src.is_some(),
            tier: j.tier(),
        };
        if let Some(r) = scheduler::never_fits(offer, &probe) {
            pass.fail.push((j.id.clone(), r));
            continue;
        }
        // Secret-env (SE6): a secret plan goes only to a node offering
        // `secret-env` right now; it waits on this server otherwise.
        if !plan.secret_env.is_empty() && !offer.features.iter().any(|f| f == k2_core::compute::proto::secret::FEATURE_SECRET_ENV) {
            wait(secret_feature_wait(), &mut pass);
            continue;
        }
        let class = pshare::JobClass::parse(&j.class).unwrap_or_default();
        let ask = share::ShareAsk { cpu_millis: plan.limits.cpu_millis, mem_bytes: plan.limits.mem_bytes, max_secs: plan.limits.max_secs, exclusive: j.exclusive, class };
        if let Some((code, msg)) = share::share_refusal(&view.limits, &ask) {
            pass.fail.push((j.id.clone(), frames::Refusal::new(code, msg)));
            continue;
        }
        if let Some(b) = &blocked {
            wait(b.clone(), &mut pass);
            continue;
        }
        // V71: `--any` waits on this server a while for a node that can
        // start it now, before it joins a node's queue.
        let since = if j.ctl.queued_at > 0 { j.ctl.queued_at } else { j.created_at };
        if j.ctl.any.is_some() && now - since < t.any_hold_secs && !share::starts_now(view, plan.limits.cpu_millis, plan.limits.mem_bytes) {
            wait(
                frames::Refusal::new("any_hold", format!("waiting up to {}s on this server for a node that can start it now", t.any_hold_secs)),
                &mut pass,
            );
            continue;
        }
        if room == 0 {
            wait(
                frames::Refusal::new(pshare::codes::TENANT_QUEUE_FULL, "this server's queue on the node is full; it goes in when a place opens"),
                &mut pass,
            );
            continue;
        }
        room -= 1;
        pass.send.push(j.id.clone());
    }
    pass
}

/// The scheduler's current view of a node (for display): position and
/// reason per queued job.
pub fn decision_for(node_id: &str) -> scheduler::Decision {
    let offer = live_view(node_id).2;
    let last = with_engine(|e| e.last_ws.get(node_id).cloned());
    let db = k2_core::db::shared();
    let conn = db.lock();
    let pause = store::node_by_id(&conn, node_id).and_then(|n| n.controller_pause);
    if let Some((o, v)) = offer.as_ref().filter(|o| share::is_shared(o)).and_then(|o| o.share.as_ref().map(|v| (o, v))) {
        let pass = shared_pass(&conn, node_id, o, v, last.as_deref(), pause.as_deref());
        let mut waiting = pass.waiting;
        for (id, r) in pass.fail {
            waiting.insert(id, (0, r));
        }
        let now = compute::now();
        for j in store::jobs_in_states(&conn, node_id, &["queued"]) {
            if let Some(nb) = j.ctl.not_before.filter(|nb| *nb > now) {
                let pos = waiting.len() + 1;
                waiting.insert(j.id.clone(), (pos, frames::Refusal::new("retry_backoff", format!("retrying after an infrastructure failure in {}s", nb - now))));
            }
        }
        return scheduler::Decision { place: None, waiting };
    }
    let (queue, load) = queued_view(&conn, node_id);
    let caps: HashMap<String, u32> = store::grants(&conn, Some(node_id), None)
        .into_iter()
        .map(|g| (g.workspace_id, g.max_parallel.max(1) as u32))
        .collect();
    let mut d = scheduler::decide(compute::now(), &queue, last.as_deref(), &load, &|ws| caps.get(ws).copied().unwrap_or(1), offer.as_ref(), pause.as_deref());
    let now = compute::now();
    for j in store::jobs_in_states(&conn, node_id, &["queued"]) {
        if let Some(nb) = j.ctl.not_before.filter(|nb| *nb > now) {
            let pos = d.waiting.len() + 1;
            d.waiting.insert(
                j.id.clone(),
                (pos, frames::Refusal::new("retry_backoff", format!("retrying after an infrastructure failure in {}s", nb - now))),
            );
        }
    }
    d
}

/// Run `f` holding the placement lock `kick` holds, so no placement can
/// interleave (the cancel route re-reads and decides under it).
pub fn with_kick_lock<T>(f: impl FnOnce() -> T) -> T {
    let _k = KICK.lock().unwrap_or_else(|p| p.into_inner());
    f()
}

/// Send this server's queued jobs into a shared node's queue (§6).
fn kick_shared(node_id: &str, tx: &mpsc::UnboundedSender<Out>, offer: &Offer) {
    let Some(view) = offer.share.as_ref() else { return };
    let last = with_engine(|e| e.last_ws.get(node_id).cloned());
    let now = compute::now();
    let pass = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let pause = store::node_by_id(&conn, node_id).and_then(|n| n.controller_pause);
        let pass = shared_pass(&conn, node_id, offer, view, last.as_deref(), pause.as_deref());
        for (id, (_, why)) in &pass.waiting {
            if let Some(j) = store::job_by_id(&conn, id).filter(|j| j.state == "queued" && j.reason.as_deref() != Some(why.code.as_str())) {
                store::set_job_reason(&conn, &j.id, Some(&why.code), Some(&why.message));
            }
        }
        pass
    };
    let mut failed = Vec::new();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for (id, why) in &pass.fail {
            // Compare-and-set: a cancel that won keeps its `cancelled`.
            if store::set_job_state_if(&conn, id, "queued", JobState::Failed, Some(&why.code), Some(&why.message), now) == Ok(true) {
                let ws = store::job_by_id(&conn, id).map(|j| j.workspace_id);
                store::record_event(&conn, "controller", "fail", Some(node_id), ws.as_deref(), Some(id), serde_json::json!({"reason": why.code, "detail": why.message}));
                failed.push(id.clone());
            }
        }
    }
    for id in &failed {
        finalize(id);
    }
    let mut sent_any = false;
    for id in pass.send {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Some(job) = store::job_by_id(&conn, &id) else { continue };
        let Some(plan) = job.plan() else { continue };
        // The tier only to a node offering `tiers` right now (T11).
        let Some((job, plan)) = fit_plan_to(&conn, job, plan, &offer.features) else { continue };
        let fence = Fence { job_id: job.id.clone(), attempt: job.attempt as u32, generation: job.generation as u32, plan_digest: job.plan_digest.clone() };
        if store::assign_if_queued(&conn, &job.id, now) != Ok(true) {
            continue;
        }
        if tx.send(Out::Frame(Box::new(Frame::Assign(Assign { fence, plan })))).is_err() {
            let _ = store::set_job_state(&conn, &job.id, JobState::Queued, Some("node_offline"), None, now);
            return;
        }
        store::record_event(
            &conn,
            "controller",
            "assign",
            Some(node_id),
            Some(&job.workspace_id),
            Some(&job.id),
            serde_json::json!({"generation": job.generation, "shared": true, "class": job.class}),
        );
        drop(conn);
        with_engine(|e| e.last_ws.insert(node_id.to_string(), job.workspace_id.clone()));
        sent_any = true;
    }
    if sent_any || !failed.is_empty() {
        notify_change();
    }
}

/// Try to place one queued job on `node_id` (a shared node: every job its
/// share has room for goes into the node's queue).
pub fn kick(node_id: &str) {
    let _k = KICK.lock().unwrap_or_else(|p| p.into_inner());
    let Some((tx, offer)) = with_engine(|e| e.conns.get(node_id).map(|c| (c.tx.clone(), c.offer.clone()))) else { return };
    if let Some(o) = offer.clone().filter(share::is_shared) {
        return kick_shared(node_id, &tx, &o);
    }
    let decision = decision_for(node_id);
    let failed = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut failed = Vec::new();
        for (id, (_, why)) in &decision.waiting {
            if let Some(j) = store::job_by_id(&conn, id) {
                // Over the node's caps (queued before this check — e.g. the
                // first kick after 0.46.1 installs — or the node's policy
                // shrank): fail it with the caps in the detail rather than
                // queue forever. Compare-and-set: a cancel that won keeps
                // its `cancelled`.
                if j.state == "queued" && why.code == scheduler::EXCEEDS_NODE_CAPS {
                    if store::set_job_state_if(&conn, id, "queued", JobState::Failed, Some(&why.code), Some(&why.message), compute::now()) == Ok(true) {
                        store::record_event(
                            &conn,
                            "controller",
                            "fail",
                            Some(node_id),
                            Some(&j.workspace_id),
                            Some(id),
                            serde_json::json!({"reason": why.code, "detail": why.message}),
                        );
                        failed.push(id.clone());
                    }
                    continue;
                }
                if j.state == "queued" && j.reason.as_deref() != Some(why.code.as_str()) {
                    store::set_job_reason(&conn, id, Some(&why.code), Some(&why.message));
                }
            }
        }
        failed
    };
    // A job failed here never ran, but its requester still gets its one
    // `[compute]` message (detached, or attached and no longer followed).
    for id in &failed {
        finalize(id);
    }
    if !failed.is_empty() {
        notify_change();
    }
    let Some(id) = decision.place else { return };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Some(job) = store::job_by_id(&conn, &id) else { return };
    let Some(plan) = job.plan() else { return };
    // Secret-env (SE6): never assign a secret plan to a node whose live
    // offer lacks `secret-env` (it would refuse it at assign anyway).
    if !plan.secret_env.is_empty() && !offer.as_ref().is_some_and(|o| o.features.iter().any(|f| f == k2_core::compute::proto::secret::FEATURE_SECRET_ENV)) {
        let w = secret_feature_wait();
        if job.reason.as_deref() != Some(w.code.as_str()) {
            store::set_job_reason(&conn, &job.id, Some(&w.code), Some(&w.message));
        }
        return;
    }
    let features = offer.as_ref().map(|o| o.features.clone()).unwrap_or_default();
    let Some((job, plan)) = fit_plan_to(&conn, job, plan, &features) else { return };
    let fence = Fence { job_id: job.id.clone(), attempt: job.attempt as u32, generation: job.generation as u32, plan_digest: job.plan_digest.clone() };
    // Compare-and-set: the decision was read under a different DB lock,
    // and a cancel may have ended the job since. Never assign over it.
    if store::assign_if_queued(&conn, &job.id, compute::now()) != Ok(true) {
        return;
    }
    if tx.send(Out::Frame(Box::new(Frame::Assign(Assign { fence, plan })))).is_err() {
        let _ = store::set_job_state(&conn, &job.id, JobState::Queued, Some("node_offline"), None, compute::now());
        return;
    }
    store::record_event(&conn, "controller", "assign", Some(node_id), Some(&job.workspace_id), Some(&job.id), serde_json::json!({"generation": job.generation}));
    drop(conn);
    with_engine(|e| e.last_ws.insert(node_id.to_string(), job.workspace_id.clone()));
    notify_change();
}

/// The stored plan fitted to the node's features before it is sent (T11,
/// [`crate::compute_place::fit_plan`]): no field the node doesn't know (a
/// node rolled back to an older k2-node would refuse the digest), the tier
/// for one that takes it.
/// A changed plan is written back (JSON and digest) while the row is still
/// queued; `None` when it no longer is (a cancel won): don't send.
fn fit_plan_to(conn: &rusqlite::Connection, job: store::JobRow, plan: frames::Plan, features: &[String]) -> Option<(store::JobRow, frames::Plan)> {
    match crate::compute_place::fit_plan(&plan, &job, features) {
        None => Some((job, plan)),
        Some(p) => {
            if store::rewrite_queued_plan(conn, &job.id, &p, &job.ctl) != Ok(true) {
                return None;
            }
            let job = store::job_by_id(conn, &job.id)?;
            Some((job, p))
        }
    }
}

/// A node-queued job a newer `--supersede` replaced, whose cancel the
/// node just answered: its end reason and detail. Only for our own "only
/// if not started" cancel (`pending::CANCEL`), never another end.
fn replaced_end(job: &store::JobRow, asked: Option<&str>) -> Option<(&'static str, Option<String>)> {
    let by = job.ctl.replaced_by.as_deref().filter(|_| asked == Some(pending::CANCEL))?;
    Some((tiers::codes::REPLACED, Some(format!("replaced by job {}", compute::message::short(by)))))
}

/// Why a secret job waits for its node (SE6).
fn secret_feature_wait() -> frames::Refusal {
    frames::Refusal::new(
        k2_core::compute::proto::secret::codes::FEATURE_MISSING,
        "the node doesn't take --secret jobs right now (k2-node 0.46.4 on Linux with the job wall, and trust 'own' for this server)",
    )
}

/// Ask the node to stop a job (cancel route).
pub fn send_cancel(node_id: &str, job_id: &str, generation: i64) -> bool {
    send_to(node_id, Frame::Cancel(Cancel { job_id: job_id.into(), generation: generation as u32, reason: "cancelled".into() }))
}

/// Close a node's live connection without revoking it (the node redials).
/// Used by the reconnect contract test to stand in for a network drop.
#[allow(dead_code)]
pub fn drop_connection(node_id: &str) -> bool {
    let tx = with_engine(|e| e.conns.get(node_id).map(|c| c.tx.clone()));
    tx.is_some_and(|tx| tx.send(Out::Close).is_ok())
}

/// Revoke a live node: signed `revoked`, then the socket closes (CN5).
pub fn revoke(node_id: &str) {
    send_to(node_id, Frame::Revoked(Revoked { reason: format!("removed by {}", controller_label()) }));
    let tx = with_engine(|e| e.conns.get(node_id).map(|c| c.tx.clone()));
    if let Some(tx) = tx {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(800));
            let _ = tx.send(Out::Close);
        });
    }
}

/// Start the compute ticker if it isn't running (any compute route or
/// node attach). Needs a tokio runtime; a no-op without one.
pub fn ensure_ticker_started() {
    if tokio::runtime::Handle::try_current().is_ok() {
        ensure_ticker();
    }
}

/// V77: the ticker runs from daemon boot (the node-admin watch calls this
/// every minute, the first time at boot) when compute is on and any node
/// row exists, so queue timeouts, lost nodes, share expiry and the
/// servers mirror never wait for a route call or an attach.
pub fn ensure_ticker_at_boot() {
    if !compute::enabled() || tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    // Before boot opened the database there is nothing to check yet (the
    // watch calls again); never panic here.
    let Some(db) = k2_core::db::try_shared() else { return };
    let any = {
        let conn = db.lock();
        !store::list_nodes(&conn, true).is_empty()
    };
    if any {
        ensure_ticker();
    }
}

/// Is the compute ticker running (tests, `status`)?
pub fn ticker_running() -> bool {
    TICKER.lock().unwrap_or_else(|p| p.into_inner()).as_ref().is_some_and(|h| !h.is_finished())
}

/// Periodic upkeep: placements, the never-stuck sweep (queue timeouts,
/// moves, lost nodes), metering, log retention.
fn ensure_ticker() {
    let mut g = TICKER.lock().unwrap_or_else(|p| p.into_inner());
    if g.as_ref().is_some_and(|h| !h.is_finished()) {
        return;
    }
    *g = Some(tokio::spawn(async {
            let every = Duration::from_millis(compute::retry::tuning().tick_millis.max(50));
            let mut tick = tokio::time::interval(every);
            let mut last_daily: Option<Instant> = None;
            loop {
                tick.tick().await;
                let daily = last_daily.is_none_or(|t| t.elapsed() > Duration::from_secs(86_400));
                if daily {
                    last_daily = Some(Instant::now());
                }
                let _ = tokio::task::spawn_blocking(move || {
                    let ids: Vec<(String, &'static str, u64, u64)> = with_engine(|e| {
                        e.conns
                            .iter()
                            .map(|(id, c)| (id.clone(), c.route, c.bytes_in.swap(0, Ordering::Relaxed), c.bytes_out.swap(0, Ordering::Relaxed)))
                            .collect()
                    });
                    for (id, route, bin, bout) in &ids {
                        kick(id);
                        if *bin > 0 || *bout > 0 {
                            let db = k2_core::db::shared();
                            let conn = db.lock();
                            store::add_usage(&conn, id, &day(), route, *bin, *bout);
                        }
                    }
                    if daily {
                        logs::sweep(14);
                        sweep_artifact_cache();
                    }
                    // Machines switched off in Settings: remove once idle.
                    crate::compute_routes::sweep_retiring();
                    let now = compute::now();
                    crate::compute_place::note_tick(now);
                    crate::compute_place::sweep(now);
                    // Shared nodes: expiry notes, the owner's server lists.
                    crate::compute_place::sweep_shares(now);
                })
                .await;
            }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::compute::proto::frames::{JobLimits, Plan};

    fn job_on(conn: &rusqlite::Connection, id: &str, node: &str) -> store::JobRow {
        let plan = Plan {
            class: None,
            session_id: None,
            est_secs: None,
            est_p90_secs: None,
            ws_max_running: None,
            job_id: id.into(),
            node_id: node.into(),
            workspace_id: "ws".into(),
            workspace_label: "ws".into(),
            requested_by: "owner".into(),
            argv: vec!["true".into()],
            env: BTreeMap::new(),
            cwd: None,
            limits: JobLimits { max_secs: 60, cpu_millis: None, mem_bytes: None, disk_bytes: 1, log_cap_bytes: 1 },
            src: None,
            exclusive: false,
            created_at: 10,
            needs: Vec::new(),
            artifacts: Vec::new(),
            stall_secs: None,
            secret_env: Vec::new(),
            tier: None,
        };
        store::insert_job(
            conn,
            &store::NewJob { plan: &plan, client_job_id: id, session_id: None, request_sha256: "h", detach: false, retry_interrupted: false, ctl: Default::default() },
        )
        .expect("insert job")
    }

    /// §6: a shared node gets this server's jobs in its own order:
    /// workspaces take turns (after the last served), FIFO inside each.
    #[test]
    fn shared_nodes_get_jobs_in_workspace_rotation() {
        let mk = |id: &str, ws: &str, at: i64| store::JobRow { id: id.into(), workspace_id: ws.into(), created_at: at, ..Default::default() };
        let jobs = vec![mk("a1", "a", 1), mk("a2", "a", 2), mk("b1", "b", 3), mk("c1", "c", 4)];
        let ids = |v: Vec<store::JobRow>| v.into_iter().map(|j| j.id).collect::<Vec<_>>();
        assert_eq!(ids(rotation_order(jobs.clone(), None)), ["a1", "b1", "c1", "a2"]);
        assert_eq!(ids(rotation_order(jobs, Some("a"))), ["b1", "c1", "a1", "a2"]);
    }

    /// Job tiers T2: the send order into a shared node is tier-first: CI,
    /// then Priority, each oldest first, then the workspace rotation. A
    /// share's room (`max_running + max_queued`) is cut from the front of
    /// this order, so a guest with two Normal jobs waiting and room for two
    /// sends a new CI job first instead of leaving it `tenant_queue_full`.
    #[test]
    fn the_send_order_into_a_shared_node_is_tier_first() {
        let mk = |id: &str, ws: &str, at: i64, tier: &str| store::JobRow {
            id: id.into(),
            workspace_id: ws.into(),
            created_at: at,
            tier: tier.into(),
            ..Default::default()
        };
        let ids = |v: Vec<store::JobRow>| v.into_iter().map(|j| j.id).collect::<Vec<_>>();
        let jobs = vec![mk("n1", "a", 1, "normal"), mk("n2", "b", 2, "normal"), mk("p1", "a", 3, "priority"), mk("c1", "b", 4, "ci"), mk("c0", "a", 5, "ci")];
        let order = ids(rotation_order(jobs, Some("a")));
        assert_eq!(order, ["c1", "c0", "p1", "n2", "n1"], "Normal still takes turns after the last-served workspace");
        assert_eq!(&order[..2], ["c1", "c0"], "room for two: both CI jobs go first");
    }

    /// r8 (2026-10-22): a job its requester cancelled sends no `[compute]`
    /// completion message (they just did it, and wait on nothing); a job
    /// someone else cancelled still tells its requester.
    #[test]
    fn a_job_its_requester_cancelled_sends_no_completion_message() {
        let _db = k2_core::db::scoped_for_test();
        let db = k2_core::db::shared();
        let mk = |id: &str, sid: &str, by_requester: bool| {
            let conn = db.lock();
            let j = job_on(&conn, id, "n1");
            conn.execute("UPDATE compute_jobs SET session_id = ?2, detach = 1 WHERE id = ?1", [&j.id, &sid.to_string()]).expect("session");
            if by_requester {
                let ctl = store::JobCtl { cancelled_by_requester: true, ..Default::default() };
                store::set_job_ctl(&conn, &j.id, &ctl).expect("ctl");
            }
            let _ = store::set_job_state(&conn, &j.id, JobState::Cancelled, Some("cancelled"), None, 30);
            j.id
        };
        {
            let conn = db.lock();
            store::insert_pending_node(&conn, "n1", "n1", "fp", "pem", &BTreeMap::new(), "owner", "123456", 1).expect("node");
        }
        let (mine, theirs) = ("sess-requester-cancel", "sess-owner-cancel");
        test_sink_register(mine);
        test_sink_register(theirs);
        let own = mk("eeeeeeee0123456789abcdef01234567", mine, true);
        let other = mk("ffffffff0123456789abcdef01234567", theirs, false);
        finalize(&own);
        finalize(&other);
        let t0 = std::time::Instant::now();
        while test_sink_peek(theirs).is_empty() {
            assert!(t0.elapsed() < Duration::from_secs(10), "the requester of a job someone else cancelled is told");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(test_sink_take(theirs)[0].contains("cancelled"));
        assert_eq!(test_sink_take(mine), Vec::<String>::new(), "no message for a job its requester cancelled");
    }

    /// k2n4, 2026-10-21: a running job whose cancel was asked for, that
    /// then comes back `interrupted/node_restart` (a full disk lost its
    /// end; the node was restarted), ends `cancelled` and is never sent
    /// again, even with retries and `retry_interrupted` on. Without a
    /// pending cancel the same interruption is retried as before.
    #[test]
    fn a_cancel_requested_job_that_comes_back_interrupted_ends_cancelled() {
        let _db = k2_core::db::scoped_for_test();
        let db = k2_core::db::shared();
        let mk = |id: &str, cancel: bool| {
            let conn = db.lock();
            let j = job_on(&conn, id, "n1");
            assert!(store::assign_if_queued(&conn, &j.id, 20).expect("assign"));
            let ctl = store::JobCtl { retries: 2, ..Default::default() };
            store::set_job_ctl(&conn, &j.id, &ctl).expect("ctl");
            conn.execute("UPDATE compute_jobs SET retry_interrupted = 1 WHERE id = ?1", [&j.id]).expect("retry_interrupted");
            if cancel {
                store::set_job_reason(&conn, &j.id, Some("cancel_requested"), None);
            }
            store::job_by_id(&conn, &j.id).expect("row")
        };
        {
            let conn = db.lock();
            store::insert_pending_node(&conn, "n1", "n1", "fp", "pem", &BTreeMap::new(), "owner", "123456", 1).expect("node");
        }
        let job = mk("cccccccc0123456789abcdef01234567", true);
        apply_observation("n1", &job.id, job.generation as u32, JobState::Running, None, None, None, None, None);
        let row = store::job_by_id(&db.lock(), &job.id).expect("row");
        assert_eq!((row.state.as_str(), row.reason.as_deref()), ("running", Some("cancel_requested")), "the cancel stays owed while it runs");
        apply_observation("n1", &job.id, job.generation as u32, JobState::Interrupted, Some("node_restart"), None, None, None, None);
        let row = store::job_by_id(&db.lock(), &job.id).expect("row");
        assert_eq!((row.state.as_str(), row.reason.as_deref()), ("cancelled", Some("cancelled")), "{row:?}");
        assert_eq!(row.generation, job.generation, "not re-dispatched");
        assert!(row.detail.as_deref().is_some_and(|d| d.contains("interrupted/node_restart")), "{:?}", row.detail);
        assert!(row.ended_at.is_some());
        // The control: no cancel owed, the interruption is retried.
        let other = mk("dddddddd0123456789abcdef01234567", false);
        apply_observation("n1", &other.id, other.generation as u32, JobState::Interrupted, Some("node_restart"), None, None, None, None);
        let row = store::job_by_id(&db.lock(), &other.id).expect("row");
        assert_eq!(row.state.as_str(), "queued", "retried: {row:?}");
        assert!(row.attempt > other.attempt, "a new attempt: {row:?}");
    }

    /// C11: a job whose cancel was asked for while assigned, and that the
    /// node then refused, ends `cancelled` instead of going back to the
    /// queue (where it would run later).
    #[test]
    fn a_cancel_requested_job_the_node_refuses_ends_cancelled() {
        let _db = k2_core::db::scoped_for_test();
        let db = k2_core::db::shared();
        let job = {
            let conn = db.lock();
            store::insert_pending_node(&conn, "n1", "n1", "fp", "pem", &BTreeMap::new(), "owner", "123456", 1).expect("node");
            let j = job_on(&conn, "aaaaaaaa0123456789abcdef01234567", "n1");
            assert!(store::assign_if_queued(&conn, &j.id, 20).expect("assign"));
            store::set_job_reason(&conn, &j.id, Some("cancel_requested"), None);
            j
        };
        apply_observation("n1", &job.id, job.generation as u32, JobState::Queued, Some("parallel_full"), None, None, None, None);
        let row = store::job_by_id(&db.lock(), &job.id).expect("row");
        assert_eq!((row.state.as_str(), row.reason.as_deref()), ("cancelled", Some("cancelled")));
        assert!(row.ended_at.is_some());
        // Without a pending cancel, a refusal re-queues as before.
        let other = {
            let conn = db.lock();
            let j = job_on(&conn, "bbbbbbbb0123456789abcdef01234567", "n1");
            assert!(store::assign_if_queued(&conn, &j.id, 20).expect("assign"));
            j
        };
        apply_observation("n1", &other.id, other.generation as u32, JobState::Queued, Some("parallel_full"), None, None, None, None);
        let row = store::job_by_id(&db.lock(), &other.id).expect("row");
        assert_eq!((row.state.as_str(), row.reason.as_deref()), ("queued", Some("parallel_full")));
    }

    /// G1 (plan-ci-release-on-k2-compute-v1): the release nodes reach
    /// z3mbpZ only through the relay and the Linux AppImage is 105 MB.
    /// The relay keeps its 100 MB default; `over_relay` lifts it to the
    /// node's own 500 MB collection cap, never past it. LAN, loopback and
    /// unknown routes were never relay-capped.
    #[test]
    fn relay_pulls_keep_the_100_mb_cap_unless_the_caller_says_over_relay() {
        let mb = 1u64 << 20;
        let appimage = 105 * mb;
        let (code, hint) = pull_cap_refusal(Some("relay"), false, appimage).expect("105 MB over the relay is refused by default");
        assert_eq!(code, "relay_cap");
        assert!(hint.contains("--over-relay") && hint.contains("105 MB") && hint.contains("100 MB"), "{hint}");
        assert_eq!(pull_cap_refusal(Some("relay"), false, 100 * mb), None, "exactly the cap passes");
        assert!(pull_cap_refusal(Some("relay"), false, 100 * mb + 1).is_some());
        assert_eq!(pull_cap_refusal(Some("relay"), true, appimage), None, "--over-relay lets the AppImage through");
        assert_eq!(pull_cap_refusal(Some("relay"), true, 500 * mb), None);
        let (code, hint) = pull_cap_refusal(Some("relay"), true, 500 * mb + 1).expect("never past the LAN cap");
        assert_eq!(code, "relay_cap");
        assert!(!hint.contains("--over-relay"), "the flag can't help here: {hint}");
        for route in [Some("lan"), Some("loopback"), None] {
            for over in [false, true] {
                assert_eq!(pull_cap_refusal(route, over, appimage), None, "{route:?}");
                assert_eq!(pull_cap_refusal(route, over, 500 * mb), None, "{route:?}");
                assert!(pull_cap_refusal(route, over, 500 * mb + 1).is_some(), "{route:?}");
            }
        }
        assert_eq!(artifact_pull_cap(Some("relay"), false), results::RELAY_MAX_FILE_BYTES);
        assert_eq!(artifact_pull_cap(Some("relay"), true), results::MAX_FILE_BYTES);
    }

    /// A background pull (`wait=`) hands its error to the next ask once,
    /// keeps nothing behind, and a later ask starts a new pull.
    #[test]
    fn a_background_pull_reports_its_error_once_and_leaves_nothing_behind() {
        let _db = k2_core::db::scoped_for_test();
        let db = k2_core::db::shared();
        let j = {
            let conn = db.lock();
            job_on(&conn, "abababab0123456789abcdef01234567", "n-gone")
        };
        let info = ArtifactInfo { name: "dist/k2.AppImage".into(), sha256: "c".repeat(64), bytes: 105 << 20 };
        let key = artifact_cache_path(&j.id, j.generation, &info.sha256);
        for round in 0..2 {
            let r = fetch_artifact_wait(&j, &info, true, Duration::from_secs(5), Duration::from_secs(10));
            match r {
                Err((code, _)) => assert_eq!(code, "node_offline", "round {round}"),
                Ok(other) => panic!("round {round}: an offline node can't send anything: {other:?}"),
            }
            assert!(with_bg(|m| !m.contains_key(&key)), "round {round}: the finished pull is gone once reported");
            assert!(with_pulls(|m| m.values().all(|p| p.dst != key)), "round {round}: no transfer left open");
        }
    }
}
