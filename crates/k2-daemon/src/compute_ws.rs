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
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use k2_core::compute::proto::crypto::{self, SigningKey};
use k2_core::compute::proto::frames::{
    self, Assign, Cancel, Challenge, Direction, EnrollChallenge, EnrollDone, Fence, Frame, HandshakeFrame,
    JobState, JobsSnapshot, LogChunk, Need, Offer, ReceiptFrame, Refused, ResumeHint, Revoked, StateUpdate,
    Transfer, TransferFailed, TransferKind, Welcome,
};
use k2_core::compute::proto::pairing;
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
static TICKER: OnceLock<()> = OnceLock::new();

fn with_engine<T>(f: impl FnOnce(&mut Engine) -> T) -> T {
    let mut g = ENGINE.lock().unwrap_or_else(|p| p.into_inner());
    f(g.get_or_insert_with(Engine::default))
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
    if req.protocol < frames::PROTOCOL {
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
    let binding = req.code_binding.trim().to_ascii_lowercase();
    let entry = match compute::enroll::take(&binding) {
        Ok(e) => e,
        Err(r) => {
            audit("node", "enroll_refused", None, serde_json::json!({"reason": r.code(), "route": route_label(ingress), "fp": node_fp}));
            let msg = match r {
                compute::enroll::CodeRefusal::LockedOut => "too many wrong codes: every waiting code was voided; ask the owner for a new one",
                compute::enroll::CodeRefusal::Invalid => "the enroll code is wrong, used, or expired",
            };
            return refuse(&mut ws, r.code(), msg).await;
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
        let r = store::insert_pending_node(
            &conn,
            &node_id,
            &entry.name,
            &node_fp,
            &req.node_public_key_pem,
            &labels,
            &entry.minted_by,
            &sas,
            compute::now(),
        );
        if r.is_ok() {
            store::record_event(
                &conn,
                &entry.minted_by,
                "enroll",
                Some(&node_id),
                None,
                None,
                serde_json::json!({"name": entry.name, "fp": node_fp, "route": route_label(ingress), "version": req.node_version}),
            );
        }
        r
    };
    if let Err(e) = inserted {
        let code = if e.contains("fingerprint") { "already_enrolled" } else { "name_taken" };
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
    if hello.protocol < frames::PROTOCOL {
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
        let cancels: Vec<Frame> = active
            .iter()
            .filter(|j| j.reason.as_deref() == Some("cancel_requested"))
            .map(|j| Frame::Cancel(Cancel { job_id: j.id.clone(), generation: j.generation as u32, reason: "cancelled".into() }))
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
    k2_core::agent_hooks::emit(k2_core::agent_hooks::HookEvent::SyncSettings, serde_json::json!({"compute": "node_online"}));
    notify_change();

    let (mut sink, mut source) = ws.split();
    // Frames are handled in order on one blocking worker per connection.
    let (work_tx, work_rx) = std::sync::mpsc::channel::<Frame>();
    let worker_node = node_id.clone();
    let worker_route = route;
    let worker = tokio::task::spawn_blocking(move || frame_worker(worker_node, worker_route, work_rx));

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

fn frame_worker(node_id: String, route: &'static str, rx: std::sync::mpsc::Receiver<Frame>) {
    let mut tallies: HashMap<(String, u32), LogTally> = HashMap::new();
    let mut snapshot_seen: HashSet<String> = HashSet::new();
    while let Ok(f) = rx.recv() {
        match f {
            Frame::Offer(o) => on_offer(&node_id, o),
            Frame::JobsSnapshot(s) => on_snapshot(&node_id, s, &mut snapshot_seen),
            Frame::Need(n) => on_need(&node_id, route, n),
            Frame::State(s) => {
                if let Some(t) = tallies.remove(&(s.job_id.clone(), s.generation)) {
                    flush_tally(&s.job_id, &t);
                }
                on_state(&node_id, s);
            }
            Frame::Log(l) => on_log(&mut tallies, l),
            Frame::Receipt(r) => on_receipt(&node_id, r),
            Frame::Goodbye(g) => audit("node", "goodbye", Some(&node_id), serde_json::json!({"reason": g.reason})),
            other => log_debug!("[compute] node {node_id} sent {}", other.kind()),
        }
    }
    for ((job, _), t) in tallies {
        flush_tally(&job, &t);
    }
}

fn flush_tally(job: &str, t: &LogTally) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::set_job_log(&conn, job, t.seq, t.bytes);
}

fn on_offer(node_id: &str, o: Offer) {
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::store_offer(&conn, node_id, &o, compute::now());
    }
    with_engine(|e| {
        if let Some(c) = e.conns.get_mut(node_id) {
            c.offer = Some(o);
        }
    });
    kick(node_id);
    notify_change();
}

fn on_log(tallies: &mut HashMap<(String, u32), LogTally>, l: LogChunk) {
    let Ok(data) = crypto::unb64(&l.data) else { return };
    if data.len() > frames::MAX_LOG_CHUNK {
        return;
    }
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
fn apply_observation(
    node_id: &str,
    job_id: &str,
    generation: u32,
    state: JobState,
    reason: Option<&str>,
    detail: Option<&str>,
    exit: Option<&frames::ExitInfo>,
    src: Option<&frames::SrcRan>,
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
    if state == JobState::Queued {
        // The node refused the assign right now; it waits for a fresh offer.
        let _ = store::set_job_state(&conn, job_id, JobState::Queued, reason, detail, now);
        return;
    }
    // Keep a pending cancel visible until the node reports the end.
    let keep_cancel = job.reason.as_deref() == Some("cancel_requested") && !state.is_terminal();
    let reason = if keep_cancel { Some("cancel_requested") } else { reason };
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
    // One automatic retry, only for an interruption, only when asked.
    if state == JobState::Interrupted && job.retry_interrupted && job.attempt == 1 {
        let _ = store::requeue_new_attempt(&conn, job_id, "retry_interrupted");
        return;
    }
    drop(conn);
    finalize(job_id);
}

/// A job reached its end: message the requesting session once (detached).
pub fn finalize(job_id: &str) {
    let (job, node_name, ws_path) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Some(job) = store::job_by_id(&conn, job_id) else { return };
        // Detached jobs always get a message. An attached job gets one too
        // when nobody is following its logs any more (the agent's shell
        // tool timed out and killed `k2 compute run`): otherwise its result
        // would only ever be found by polling.
        let wants = job.detach || (job.session_id.is_some() && !follower_recent(&job.id));
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
    let text = compute::message::completion_text(
        &job.id,
        &node_name,
        &job.state,
        job.exit_code,
        job.signal,
        job.reason.as_deref(),
        secs,
    );
    let session = job.session_id.clone();
    let id = job.id.clone();
    std::thread::spawn(move || deliver_completion(&id, session.as_deref(), ws_path.as_deref(), &text));
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

fn on_state(node_id: &str, s: StateUpdate) {
    apply_observation(node_id, &s.job_id, s.generation, s.state, s.reason.as_deref(), s.detail.as_deref(), s.exit.as_ref(), s.src.as_ref());
    if s.state.is_terminal() {
        kick(node_id);
    }
    notify_change();
}

fn on_snapshot(node_id: &str, s: JobsSnapshot, seen: &mut HashSet<String>) {
    for j in &s.jobs {
        seen.insert(format!("{}#{}", j.job_id, j.generation));
        apply_observation(node_id, &j.job_id, j.generation, j.state, j.reason.as_deref(), None, j.exit.as_ref(), None);
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
            if !seen.contains(&format!("{}#{}", j.id, j.generation)) {
                let _ = store::set_job_state(&conn, &j.id, JobState::Unknown, Some("not_on_node"), None, compute::now());
                store::record_event(&conn, "controller", "job_unknown", Some(node_id), Some(&j.workspace_id), Some(&j.id), serde_json::json!({}));
                lost.push(j.id);
            }
        }
    }
    seen.clear();
    for id in lost {
        finalize(&id);
    }
    kick(node_id);
    notify_change();
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
    drop(conn);
    notify_change();
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

fn send_to(node_id: &str, f: Frame) -> bool {
    let tx = with_engine(|e| e.conns.get(node_id).map(|c| c.tx.clone()));
    tx.is_some_and(|tx| tx.send(Out::Frame(Box::new(f))).is_ok())
}

// ── placement ────────────────────────────────────────────────────────

/// Queued jobs on a node as the scheduler sees them.
pub fn queued_view(conn: &rusqlite::Connection, node_id: &str) -> (Vec<scheduler::QueuedJob>, scheduler::Load) {
    let queue = store::jobs_in_states(conn, node_id, &["queued"])
        .into_iter()
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

/// The scheduler's current view of a node (for display): position and
/// reason per queued job.
pub fn decision_for(node_id: &str) -> scheduler::Decision {
    let offer = live_view(node_id).2;
    let last = with_engine(|e| e.last_ws.get(node_id).cloned());
    let db = k2_core::db::shared();
    let conn = db.lock();
    let pause = store::node_by_id(&conn, node_id).and_then(|n| n.controller_pause);
    let (queue, load) = queued_view(&conn, node_id);
    let caps: HashMap<String, u32> = store::grants(&conn, Some(node_id), None)
        .into_iter()
        .map(|g| (g.workspace_id, g.max_parallel.max(1) as u32))
        .collect();
    scheduler::decide(&queue, last.as_deref(), &load, &|ws| caps.get(ws).copied().unwrap_or(1), offer.as_ref(), pause.as_deref())
}

/// Try to place one queued job on `node_id`.
pub fn kick(node_id: &str) {
    let _k = KICK.lock().unwrap_or_else(|p| p.into_inner());
    let Some(tx) = with_engine(|e| e.conns.get(node_id).map(|c| c.tx.clone())) else { return };
    let decision = decision_for(node_id);
    let db = k2_core::db::shared();
    let conn = db.lock();
    for (id, (_, why)) in &decision.waiting {
        if let Some(j) = store::job_by_id(&conn, id) {
            if j.state == "queued" && j.reason.as_deref() != Some(why.code.as_str()) {
                store::set_job_reason(&conn, id, Some(&why.code), Some(&why.message));
            }
        }
    }
    let Some(id) = decision.place else { return };
    let Some(job) = store::job_by_id(&conn, &id) else { return };
    let Some(plan) = job.plan() else { return };
    let fence = Fence { job_id: job.id.clone(), attempt: job.attempt as u32, generation: job.generation as u32, plan_digest: job.plan_digest.clone() };
    if store::set_job_state(&conn, &job.id, JobState::Assigned, None, None, compute::now()).is_err() {
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

/// Periodic upkeep: placements, metering, log retention.
fn ensure_ticker() {
    TICKER.get_or_init(|| {
        tokio::spawn(async {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            let mut n: u64 = 0;
            loop {
                tick.tick().await;
                n += 1;
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
                    if n % 17_280 == 1 {
                        logs::sweep(14);
                    }
                })
                .await;
            }
        });
    });
}
