//! Dialing the controller, the signed attach handshake (§7.3), one
//! session's loops, and the run loop with backoff (§11.5, §12.1).
//!
//! Every session belongs to one [`Tenant`]: one server's pin, ledger,
//! journals and hub (prd-compute-shared-queue-v1 §5). Solo mode (no
//! `servers.toml`) runs one session, the owner's, exactly as 0.46.2 did.
//! With `servers.toml`, [`run_forever`] also runs one session per server it
//! approves ([`run_servers`]): each with its own attach, offers, logs and
//! receipts, so one server going down touches nothing else.
//!
//! Per session (V51, V52): a non-owner server's `Revoked` ends only that
//! server; it gets no `Health`, and its `Admin`, `AdminKeys`, `UpdateOffer`
//! and `UpdateChunk` frames are dropped before they touch any state
//! (`not_node_owner`, once per session); only the owner's attach marks a
//! self-update healthy. A non-owner server must speak protocol 3.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use k2_node_proto::crypto::{self, SigningKey};
use k2_node_proto::frames::{
    self, Challenge, Direction, Frame, HandshakeFrame, Hello, JobState, JobsSnapshot, Proof, ReceiptFrame, Refused,
    Welcome, HANDSHAKE_SECS, MAX_FRAME_BYTES, PROTOCOL, PROTOCOL_SHARED,
};
use k2_node_proto::share::codes::{self, NOT_NODE_OWNER};
use k2_node_proto::share::{ServerEntry, ServerStatus, ServersFile, ShareState, Slot, OWNER_SLOT};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;

use crate::identity::Pin;
use crate::journal::JournalReader;
use crate::node::tenant::ShareInfo;
use crate::node::{Node, Tenant, NODE_VERSION};
use crate::status::ServerLine;

pub const ATTACH_PATH: &str = "/cli/compute/attach";
pub const ENROLL_PATH: &str = "/cli/compute/enroll";

pub type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Install the aws-lc-rs provider for rustls once.
pub fn init_tls() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

pub fn ws_config() -> WebSocketConfig {
    let mut c = WebSocketConfig::default();
    c.max_message_size = Some(MAX_FRAME_BYTES + 4096);
    c.max_frame_size = Some(MAX_FRAME_BYTES + 4096);
    c
}

/// Open a WebSocket to `base` + `path` (5 s).
pub async fn connect(base: &str, path: &str) -> Result<Ws, String> {
    init_tls();
    let url = crate::url::ws_url(base, path)?;
    let fut = tokio_tungstenite::connect_async_with_config(url.as_str(), Some(ws_config()), false);
    match tokio::time::timeout(Duration::from_secs(5), fut).await {
        Err(_) => Err(format!("{url}: connect timed out")),
        Ok(Err(e)) => Err(format!("{url}: {e}")),
        Ok(Ok((ws, _))) => Ok(ws),
    }
}

pub async fn send_text<S>(ws: &mut S, text: String) -> Result<(), String>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    ws.send(Message::Text(text)).await.map_err(|e| format!("send: {e}"))
}

pub async fn send_hs<S>(ws: &mut S, f: &HandshakeFrame) -> Result<(), String>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    send_text(ws, serde_json::to_string(f).map_err(|e| e.to_string())?).await
}

/// The next text message (pings/pongs skipped).
pub async fn recv_text<S>(ws: &mut S) -> Result<String, String>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match ws.next().await {
            None => return Err("connection closed".into()),
            Some(Err(e)) => return Err(format!("read: {e}")),
            Some(Ok(Message::Text(t))) => return Ok(t),
            Some(Ok(Message::Close(c))) => return Err(format!("closed by peer{}", c.map(|c| format!(": {}", c.reason)).unwrap_or_default())),
            Some(Ok(Message::Binary(_))) => return Err("protocol_error: binary message".into()),
            Some(Ok(_)) => continue,
        }
    }
}

pub fn parse_hs(text: &str) -> Result<HandshakeFrame, String> {
    if text.len() > MAX_FRAME_BYTES {
        return Err("protocol_error: frame too large".into());
    }
    serde_json::from_str(text).map_err(|e| format!("protocol_error: {e}"))
}

pub struct SessionStart {
    pub session: String,
    pub welcome: Welcome,
    pub controller_spki: Vec<u8>,
    /// What the server said in `Challenge.protocol`: sharing fields and
    /// features go only to ≥ `PROTOCOL_SHARED` (§10.1).
    pub protocol: u32,
}

pub enum AttachOutcome {
    Session(SessionStart),
    Refused(Refused),
    /// The node refused the server: it speaks an older protocol than this
    /// server's role needs (`server_too_old`, §4.1). Carries what it said.
    TooOld(u32),
}

/// The node side of the attach handshake. Every check fails closed.
pub async fn attach<S>(ws: &mut S, key: &SigningKey, pin: &Pin, boot_id: &str, ledger_id: &str) -> Result<AttachOutcome, String>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    attach_as(ws, key, pin, boot_id, ledger_id, 0).await
}

/// [`attach`] refusing a server below `min_protocol` before the node proves
/// itself: a non-owner server must speak protocol 3 (a 0.46.2 server
/// enrols, its SAS is confirmed, then its attach is refused
/// `server_too_old`, §4.1, V79). The owner passes 0 (any server, today).
pub async fn attach_as<S>(ws: &mut S, key: &SigningKey, pin: &Pin, boot_id: &str, ledger_id: &str, min_protocol: u32) -> Result<AttachOutcome, String>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let fut = async {
        let controller_spki = pin.controller_spki()?;
        let node_fp = key.fingerprint();
        let nonce_n = crypto::nonce_b64()?;
        send_hs(ws, &HandshakeFrame::Hello(Hello {
            node_fp: node_fp.clone(),
            protocol: PROTOCOL,
            boot_id: boot_id.to_string(),
            ledger_id: ledger_id.to_string(),
            nonce_n: nonce_n.clone(),
            node_version: NODE_VERSION.to_string(),
        }))
        .await?;
        let ch: Challenge = match parse_hs(&recv_text(ws).await?)? {
            HandshakeFrame::Challenge(c) => c,
            HandshakeFrame::Refused(r) => return Ok(AttachOutcome::Refused(r)),
            other => return Err(format!("protocol_error: expected challenge, got {other:?}")),
        };
        if !crypto::ct_eq(ch.controller_fp.as_bytes(), pin.controller_fp.as_bytes()) {
            return Err("controller fingerprint doesn't match the pin; refusing (re-pair if the controller's key really changed)".into());
        }
        if !crypto::verify_domain(
            &controller_spki,
            frames::DOMAIN_ATTACH_CONTROLLER,
            &[nonce_n.as_bytes(), node_fp.as_bytes(), ch.nonce_c.as_bytes()],
            &ch.sig,
        ) {
            return Err("controller challenge signature is bad".into());
        }
        if ch.protocol < min_protocol {
            let _ = send_hs(ws, &HandshakeFrame::Refused(Refused {
                code: k2_node_proto::share::codes::SERVER_TOO_OLD.into(),
                message: format!("this node is shared and needs a K2 server on protocol {min_protocol} (0.46.3) or newer; this server said {}: update it", ch.protocol),
            }))
            .await;
            return Ok(AttachOutcome::TooOld(ch.protocol));
        }
        let protocol = ch.protocol;
        let sig = key.sign_domain(frames::DOMAIN_ATTACH_NODE, &[ch.nonce_c.as_bytes(), pin.controller_fp.as_bytes(), nonce_n.as_bytes()])?;
        send_hs(ws, &HandshakeFrame::Proof(Proof { sig })).await?;
        let text = recv_text(ws).await?;
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("protocol_error: {e}"))?;
        if v.get("t").is_some() {
            return match parse_hs(&text)? {
                HandshakeFrame::Refused(r) => Ok(AttachOutcome::Refused(r)),
                other => Err(format!("protocol_error: expected welcome, got {other:?}")),
            };
        }
        let session = frames::session_id(&nonce_n, &ch.nonce_c);
        match frames::open(&controller_spki, Direction::ControllerToNode, &session, 0, &text) {
            Ok(Frame::Welcome(welcome)) => Ok(AttachOutcome::Session(SessionStart { session, welcome, controller_spki, protocol })),
            Ok(other) => Err(format!("protocol_error: first frame is {}", other.kind())),
            Err(e) => Err(format!("protocol_error: {e}")),
        }
    };
    match tokio::time::timeout(Duration::from_secs(HANDSHAKE_SECS), fut).await {
        Err(_) => Err("handshake timed out".into()),
        Ok(r) => r,
    }
}

/// How a session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEnd {
    Closed(String),
    ProtocolError(String),
    Revoked(String),
}

struct Track {
    reader: JournalReader,
    sent: Option<(JobState, Option<String>)>,
    hinted: bool,
    /// Assigned in this session: its server sees `running` before any
    /// later state, even when the job was over before the shipper looked
    /// (the server's start time and duration come from it).
    fresh: bool,
    ran_sent: bool,
}

type Tracks = Arc<Mutex<HashMap<(String, u32), Track>>>;

fn track(t: &Tenant, job: &str, gen: u32, since: u64, hinted: bool) -> Track {
    let mut reader = JournalReader::new(t.paths.journal(job, gen));
    reader.skip_to(since);
    Track { reader, sent: None, hinted, fresh: false, ran_sent: false }
}

/// One pass of the shipper: logs first, then state, then the receipt.
/// `Err` when the session's queue is gone.
/// `closing`: the session's last pass (its server was ended): a final
/// state goes out even when its log can't be read to the end (a wiped
/// slot), since nothing ships after this.
async fn ship_pass(node: &Node, tenant: &Tenant, tracks: &Tracks, tx: &mpsc::Sender<Frame>, closing: bool) -> Result<(), ()> {
    let keys: Vec<(String, u32)> = tracks.lock().unwrap().keys().cloned().collect();
    for key in keys {
        let Some(mut t) = tracks.lock().unwrap().remove(&key) else { continue };
        let row = tenant.ledger.lock().unwrap().get(&key.0, key.1).ok().flatten();
        let Some(row) = row else {
            if t.hinted {
                let f = Frame::State(frames::StateUpdate {
                    queue: None,
                    job_id: key.0.clone(),
                    generation: key.1,
                    state: JobState::Unknown,
                    reason: Some("not_on_node".into()),
                    detail: None,
                    at: node.now(),
                    exit: None,
                    src: None,
                    log_seq: 0,
                    outputs: None,
                });
                tx.send(f).await.map_err(|_| ())?;
            }
            continue;
        };
        loop {
            let recs = t.reader.read_more(32).unwrap_or_default();
            let n = recs.len();
            for r in recs {
                let f = Frame::Log(frames::LogChunk {
                    job_id: key.0.clone(),
                    generation: key.1,
                    seq: r.seq,
                    stream: r.stream,
                    data: crypto::b64(&r.data),
                });
                tx.send(f).await.map_err(|_| ())?;
            }
            if n < 32 {
                break;
            }
        }
        let state_key = (row.state, row.reason.clone());
        if row.state.is_terminal() {
            if t.reader.last_seq < row.log_seq && !closing {
                tracks.lock().unwrap().insert(key, t);
                continue;
            }
            if t.sent.as_ref() != Some(&state_key) {
                send_running_first(&mut t, &row, tx).await?;
                tx.send(node.row_state_frame(&row)).await.map_err(|_| ())?;
            }
            if let Some(receipt) = row.receipt.clone() {
                tx.send(Frame::Receipt(ReceiptFrame { job_id: key.0.clone(), generation: key.1, receipt })).await.map_err(|_| ())?;
                continue; // done with this attempt
            }
            t.sent = Some(state_key);
            tracks.lock().unwrap().insert(key, t);
        } else {
            if t.sent.as_ref() != Some(&state_key) {
                send_running_first(&mut t, &row, tx).await?;
                tx.send(node.row_state_frame(&row)).await.map_err(|_| ())?;
                t.sent = Some(state_key);
            }
            tracks.lock().unwrap().insert(key, t);
        }
    }
    Ok(())
}

/// A job this session assigned that ran but whose `running` the shipper
/// never sent (it was over, or finishing, before the shipper looked): send
/// `running` at its start time first, so the server has its start
/// (`startedAt`, `durationSecs`). Once per attempt.
async fn send_running_first(t: &mut Track, row: &crate::ledger::Row, tx: &mpsc::Sender<Frame>) -> Result<(), ()> {
    if row.state == JobState::Running {
        t.ran_sent = true;
        return Ok(());
    }
    let Some(started) = row.started_at else { return Ok(()) };
    if !t.fresh || t.ran_sent {
        return Ok(());
    }
    t.ran_sent = true;
    let f = Frame::State(frames::StateUpdate {
        queue: None,
        job_id: row.job_id.clone(),
        generation: row.generation,
        state: JobState::Running,
        reason: None,
        detail: None,
        at: started,
        exit: None,
        src: row.src.clone(),
        log_seq: 0,
        outputs: None,
    });
    tx.send(f).await.map_err(|_| ())
}

/// Run one established session of `tenant`'s server until it ends.
pub async fn run_session(node: Arc<Node>, tenant: Arc<Tenant>, ws: Ws, start: SessionStart) -> SessionEnd {
    run_session_on(node, tenant, ws, start).await
}

/// [`run_session`] over any WebSocket (tests use the loopback one).
pub async fn run_session_on<S>(node: Arc<Node>, tenant: Arc<Tenant>, ws: S, start: SessionStart) -> SessionEnd
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin
        + Send
        + 'static,
{
    let (sink, mut stream) = ws.split();
    let (tx, rx) = mpsc::channel::<Frame>(1024);
    let session = start.session.clone();
    let spki = start.controller_spki.clone();

    let owner = tenant.is_owner();
    tenant.protocol.store(start.protocol, std::sync::atomic::Ordering::SeqCst);
    if owner {
        let name = start.welcome.controller.trim();
        if !name.is_empty() && name.len() <= 253 {
            *node.owner_name.lock().unwrap() = Some(name.to_string());
        }
    }
    // The queue: an old (protocol < 3) owner never queues (V70 b).
    node.fairq.set_protocol(tenant.slot, Some(start.protocol));
    // Clock skew (§14): order and ETAs never depend on it; it's shown.
    if start.welcome.server_time != 0 {
        // The node's clock minus the server's: positive = the node is ahead.
        let skew = node.now() - start.welcome.server_time;
        if skew.abs() > crate::status::SKEW_WARN_SECS {
            crate::nlog!("server slot {}: this node's clock is {skew:+} s off the server's", tenant.slot);
        }
        tenant.link.lock().unwrap().skew_secs = Some(skew);
        if owner {
            node.status.lock().unwrap().clock_skew_secs = Some(skew);
        }
    }
    // Learned routes (this server's only), the confirmed flag, narrowing:
    // all `tenant`'s (V51).
    // Another server's routes are https to public hosts only, at most 8:
    // it can't make the node dial into its LAN (review B7). The owner's
    // are taken as today.
    let mut learn = Vec::new();
    for r in &start.welcome.routes {
        if crate::url::ws_url(r, ATTACH_PATH).is_err() {
            continue;
        }
        if !owner && !crate::url::tenant_route_ok(r).await {
            crate::nlog!("server slot {}: dropped route {r:?} (another server's routes are https to public hosts only)", tenant.slot);
            continue;
        }
        learn.push(r.clone());
    }
    {
        let mut lr = tenant.learned_routes.lock().unwrap();
        for r in learn {
            if lr.contains(&r) {
                continue;
            }
            if !owner && lr.len() >= crate::url::MAX_TENANT_ROUTES {
                crate::nlog!("server slot {}: dropped route {r:?} (at most {} routes per server)", tenant.slot, crate::url::MAX_TENANT_ROUTES);
                continue;
            }
            lr.push(r);
        }
    }
    node.mark_confirmed(&tenant);
    // A server whose SAS was just confirmed is active from this attach on:
    // tell the queue now, not on the servers loop's next pass, so its
    // first assign isn't refused `server_not_approved`.
    if let (Some(info), Some(status)) = (tenant.share(), tenant.status_at(node.now())) {
        node.fairq.set_share(tenant.slot, Some(crate::fairq::ShareInfo { label: info.entry.label.clone(), limits: info.entry.limits(), status }));
    }
    // Self-update health gate: this build reached its owner server (V52:
    // another server's attach says nothing about the owner's update).
    if owner {
        crate::admin::update::mark_healthy(&node.layout().home, true);
    }
    if let Some(n) = start.welcome.narrow.clone() {
        node.narrow(&tenant, n);
    }

    // Writer: seal with the per-direction sequence, ping every 20 s.
    let wnode = node.clone();
    let ping_every = node.opts.ping_every;
    let wsession = session.clone();
    // Why the writer stopped (a failed write or ping), for the session-end
    // log line: "writer ended" alone hid macOS's EHOSTUNREACH in the smoke.
    let writer_why: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let ww = writer_why.clone();
    let writer = tokio::spawn(async move {
        let why = writer_loop(wnode, ping_every, wsession, rx, sink).await;
        *ww.lock().unwrap() = Some(why);
    });
    async fn writer_loop<K>(
        wnode: Arc<Node>,
        ping_every: Duration,
        wsession: String,
        mut rx: mpsc::Receiver<Frame>,
        mut sink: K,
    ) -> String
    where
        K: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
    {
        let key = &wnode.key;
        let mut seq = 0u64;
        let mut ping = tokio::time::interval(ping_every);
        ping.tick().await;
        loop {
            tokio::select! {
                f = rx.recv() => {
                    let Some(f) = f else { return "queue closed".to_string() };
                    let text = match frames::seal(key, Direction::NodeToController, &wsession, seq, &f) {
                        Ok(t) => t,
                        Err(e) => { crate::nlog!("can't seal {}: {e}", f.kind()); continue; }
                    };
                    seq += 1;
                    if let Err(e) = sink.send(Message::Text(text)).await { return format!("write: {e}"); }
                }
                _ = ping.tick() => {
                    if let Err(e) = sink.send(Message::Ping(Vec::new())).await { return format!("ping: {e}"); }
                }
            }
        }
    }

    let epoch = tenant.hub.attach(tx.clone());
    // The node queue (S2): queued rows this server still lists may start in
    // this session; the rest end `superseded` (V47).
    node.queue_attach(&tenant, &start.welcome.resume);

    // Tracks: resume hints + every open attempt.
    let tracks: Tracks = Arc::new(Mutex::new(HashMap::new()));
    {
        let mut t = tracks.lock().unwrap();
        // A hint's id names a journal path: only a folder-safe one (no row
        // can have another, review A-disk 2).
        for h in start.welcome.resume.iter().filter(|h| frames::valid_job_id(&h.job_id)) {
            t.insert((h.job_id.clone(), h.generation), track(&tenant, &h.job_id, h.generation, h.since_seq, true));
        }
        for r in tenant.ledger.lock().unwrap().non_terminal().unwrap_or_default() {
            t.entry((r.job_id.clone(), r.generation)).or_insert_with(|| track(&tenant, &r.job_id, r.generation, 0, false));
        }
    }

    // Snapshot pages, then the first offer.
    let rows = tenant.ledger.lock().unwrap().all().unwrap_or_default();
    let jobs: Vec<_> = rows.iter().map(|r| r.to_ledger_job()).collect();
    let pages: Vec<_> = if jobs.is_empty() { vec![vec![]] } else { jobs.chunks(200).map(|c| c.to_vec()).collect() };
    let last = pages.len() - 1;
    for (i, p) in pages.into_iter().enumerate() {
        if tx.send(Frame::JobsSnapshot(JobsSnapshot { jobs: p, last: i == last })).await.is_err() {
            break;
        }
    }
    let first = node.offer(&tenant);
    let _ = tx.send(Frame::Offer(first.clone())).await;
    // Tell the owner server how the last self-update ended (it dedups).
    if owner {
        let (n, t) = (node.clone(), tx.clone());
        tokio::spawn(async move { crate::admin::courier::report_last_update(n, t).await });
    }

    // Health (node admin §10): at once, then on change (checked every
    // minute) and at least every HEALTH_SECS. The owner server's only: it
    // carries people's home paths, update state and every server's stuck
    // jobs (V52).
    let health_node = node.clone();
    let health_tx = tx.clone();
    let health = tokio::spawn(async move {
        if !health_node.opts.health || !owner {
            return;
        }
        let mut last: Option<(k2_node_proto::health::Health, std::time::Instant)> = None;
        loop {
            crate::admin::health::refresh_updates_in_background(&health_node);
            let h = crate::admin::health::collect(&health_node, false).await;
            let due = match &last {
                None => true,
                Some((prev, at)) => !crate::admin::health::same_facts(prev, &h) || at.elapsed() >= Duration::from_secs(k2_node_proto::health::HEALTH_SECS),
            };
            if due {
                if health_tx.send(Frame::Health(h.clone())).await.is_err() {
                    return;
                }
                last = Some((h, std::time::Instant::now()));
            }
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });

    // Shipper.
    let ship_node = node.clone();
    let ship_tenant = tenant.clone();
    let ship_tracks = tracks.clone();
    let ship_tx = tx.clone();
    // One pass at a time: the session's last pass must see every track
    // (a pass takes a track out while it ships it).
    let ship_lock = Arc::new(tokio::sync::Mutex::new(()));
    let ship_lock_task = ship_lock.clone();
    let shipper = tokio::spawn(async move {
        loop {
            let pass = {
                let _one = ship_lock_task.lock().await;
                ship_pass(&ship_node, &ship_tenant, &ship_tracks, &ship_tx, false).await
            };
            if pass.is_err() {
                return;
            }
            tokio::select! {
                _ = ship_tenant.ship.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            }
        }
    });

    // Offers: on change, and every OFFER_SECS regardless.
    let offer_node = node.clone();
    let offer_tenant = tenant.clone();
    let offer_tx = tx.clone();
    let offers = tokio::spawn(async move {
        let mut last_rev = first.revision;
        let mut tick = tokio::time::interval(offer_node.opts.offer_every);
        tick.tick().await;
        loop {
            let force = tokio::select! {
                _ = offer_tenant.offer_changed.notified() => false,
                _ = tick.tick() => true,
            };
            let o = offer_node.offer(&offer_tenant);
            if force || o.revision != last_rev {
                last_rev = o.revision;
                if offer_tx.send(Frame::Offer(o)).await.is_err() {
                    return;
                }
            }
        }
    });

    // Reader.
    let mut expected = 1u64; // Welcome was seq 0
    let mut serve = tenant.serve.subscribe();
    // A non-owner's owner-only frames are answered once per session (V52).
    let mut told_not_owner = false;
    // The last queue view this session got, and when (B6).
    let mut last_view: Option<(std::time::Instant, k2_node_proto::share::QueueView)> = None;
    let end = loop {
        let msg = tokio::select! {
            m = stream.next() => m,
            _ = async { while !writer.is_finished() { tokio::time::sleep(Duration::from_millis(200)).await; } } => {
                let why = writer_why.lock().unwrap().clone().unwrap_or_else(|| "stopped".into());
                break SessionEnd::Closed(format!("writer ended: {why}"));
            }
            _ = async { let _ = serve.wait_for(|on| !*on).await; } => {
                // The node owner revoked, ended or reset this server: the
                // final states and receipts still unsent go out first (a
                // stopped job must read `cancelled/share_revoked` there, not
                // lost), then a last offer says why (its share state), then
                // close.
                let _ = tokio::time::timeout(Duration::from_secs(2), async {
                    let _one = ship_lock.lock().await;
                    ship_pass(&node, &tenant, &tracks, &tx, true).await
                })
                .await;
                let _ = tx.send(Frame::Offer(node.offer(&tenant))).await;
                break SessionEnd::Closed("ended by the node owner".into());
            }
        };
        let text = match msg {
            None => break SessionEnd::Closed("connection closed".into()),
            Some(Err(e)) => break SessionEnd::Closed(format!("read: {e}")),
            Some(Ok(Message::Text(t))) => t,
            Some(Ok(Message::Close(_))) => break SessionEnd::Closed("closed by controller".into()),
            Some(Ok(Message::Binary(_))) => break SessionEnd::ProtocolError("binary message".into()),
            Some(Ok(_)) => continue,
        };
        let frame = match frames::open(&spki, Direction::ControllerToNode, &session, expected, &text) {
            Ok(f) => f,
            Err(e) => break SessionEnd::ProtocolError(e.to_string()),
        };
        expected += 1;
        // Owner-only frames from any other server: dropped before they touch
        // any state (the owner's update transfer, the executor's audit),
        // answered `not_node_owner` once per session (V52, P4).
        if !owner {
            if let Some(answer) = not_owner_answer(&node, &frame) {
                if !told_not_owner {
                    told_not_owner = true;
                    if let Some(f) = answer {
                        let _ = tx.send(f).await;
                    }
                }
                continue;
            }
        }
        match frame {
            Frame::Assign(a) => {
                // A node that lost its job wall serves only its owner: another
                // server's jobs stay queued on that server (V59). A `--dev`
                // node (a person's own test node; never an installed one,
                // and it never has a wall) is exempt so tests and the S5
                // harness can share one. The share itself (paused, ended,
                // revoked, not approved) is the node queue's to judge
                // (`on_assign`). A fence the ledger knows still reports.
                if !owner && !node.wall.is_walled() && !crate::node::dev_test_exemption(node.opts.dev, &node.layout().config) {
                    let code = codes::TENANT_WALL_MISSING;
                    let known = {
                        let l = tenant.ledger.lock().unwrap();
                        l.get(&a.fence.job_id, a.fence.generation).ok().flatten().is_some()
                            || l.latest(&a.fence.job_id).ok().flatten().is_some_and(|r| r.generation > a.fence.generation)
                    };
                    if !known {
                        let _ = tx.send(gate_frame(&node, &a.fence.job_id, a.fence.generation, code)).await;
                        continue;
                    }
                }
                if let Some((job, gen)) = node.on_assign(&tenant, a) {
                    let mut fresh = track(&tenant, &job, gen, 0, false);
                    fresh.fresh = true;
                    tracks.lock().unwrap().insert((job.clone(), gen), fresh);
                    tenant.ship.notify_one();
                }
            }
            // Secret-env (SE18): only to a job of this server waiting for it.
            Frame::SecretEnv(e) => {
                let (job, gen) = (e.job_id.clone(), e.generation);
                tenant.hub.route_secret(&job, gen, crate::node::tenant::SecretAnswer::Env(e));
            }
            Frame::SecretRefused(r) => {
                let (job, gen) = (r.job_id.clone(), r.generation);
                tenant.hub.route_secret(&job, gen, crate::node::tenant::SecretAnswer::Refused(r));
            }
            Frame::Transfer(t) => tenant.hub.route_transfer((t.job_id.clone(), t.generation, crate::node::kind_name(t.kind)), crate::node::TransferEvent::Chunk(t)),
            Frame::TransferFailed(t) => tenant.hub.route_transfer((t.job_id.clone(), t.generation, crate::node::kind_name(t.kind)), crate::node::TransferEvent::Failed(t)),
            Frame::Cancel(c) => node.cancel(&tenant, &c.job_id, c.generation, &c.reason),
            Frame::PolicyNarrow(p) => node.narrow(&tenant, p.limits),
            Frame::Admin(a) => {
                let (n, t) = (node.clone(), tx.clone());
                tokio::spawn(async move {
                    let r = crate::admin::courier::handle(n, a.cmd).await;
                    let _ = t.send(Frame::AdminResult(r)).await;
                });
            }
            Frame::AdminKeys(k) => {
                let (n, t) = (node.clone(), tx.clone());
                tokio::spawn(async move {
                    let r = crate::admin::courier::keys(n, k).await;
                    let _ = t.send(Frame::AdminResult(r)).await;
                });
            }
            Frame::ArtifactGet(g) => {
                // Streamed from a blocking thread; the bounded queue paces it.
                let (n, t) = (tenant.clone(), tx.clone());
                tokio::task::spawn_blocking(move || {
                    let known = n.ledger.lock().unwrap().get(&g.job_id, g.generation).ok().flatten().map(|r| r.outputs.map(|o| o.artifacts).unwrap_or_default());
                    crate::artifacts::answer(&n.paths.artifacts(), &g, known.as_deref(), &mut |f| t.blocking_send(f).is_ok());
                });
            }
            Frame::UpdateOffer(o) => crate::admin::courier::update_offer(&node, &tx, o),
            Frame::UpdateChunk(c) => crate::admin::courier::update_chunk(&node, &tx, c),
            Frame::QueueGet(g) => {
                // Only a protocol-3 session asks (older parsers don't know it).
                if tenant.speaks_shared() {
                    // At most one fresh view a second per session (review
                    // B6): a faster asker gets the last one again, under
                    // its own request id, and never makes the queue lock
                    // every ledger for it.
                    if let Some((_, v)) = last_view.as_ref().filter(|(at, _)| at.elapsed() < QUEUE_VIEW_EVERY) {
                        let mut v = v.clone();
                        v.request_id = g.request_id.clone();
                        let _ = tx.send(Frame::QueueView(v)).await;
                        continue;
                    }
                    let (n, t, tx2) = (node.clone(), tenant.clone(), tx.clone());
                    let v = tokio::task::spawn_blocking(move || n.fairq.view(&n, &t, &g.request_id)).await;
                    match v {
                        Ok(v) => {
                            last_view = Some((std::time::Instant::now(), v.clone()));
                            let _ = tx2.send(Frame::QueueView(v)).await;
                        }
                        Err(e) => crate::nlog!("queue view: {e}"),
                    }
                }
            }
            Frame::Revoked(r) => {
                node.revoke(&tenant, &r.reason);
                break SessionEnd::Revoked(r.reason);
            }
            Frame::Welcome(_) => break SessionEnd::ProtocolError("second welcome".into()),
            other => break SessionEnd::ProtocolError(format!("{} can't come from the controller", other.kind())),
        }
    };
    tenant.hub.detach(epoch);
    tenant.protocol.store(0, std::sync::atomic::Ordering::SeqCst);
    shipper.abort();
    offers.abort();
    health.abort();
    drop(tx);
    // Let queued frames (e.g. a goodbye) flush briefly, then stop.
    let _ = tokio::time::timeout(Duration::from_millis(200), async {
        while !writer.is_finished() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    writer.abort();
    end
}

/// A frame only the node's owner server may send, from another server:
/// `Some(answer)` (the inner `None` = nothing to say back); `None` when any
/// server may send it (V52). Nothing here touches state.
fn not_owner_answer(node: &Node, f: &Frame) -> Option<Option<Frame>> {
    use k2_node_proto::admin::{AdminRefusal, AdminResult, UpdatePhase};
    let why = "only this node's owner server sends admin commands, keys and updates";
    let refusal = || AdminRefusal::new(NOT_NODE_OWNER, why);
    match f {
        Frame::Admin(_) => Some(Some(Frame::AdminResult(AdminResult::refused("", "", &refusal())))),
        Frame::AdminKeys(_) => Some(Some(Frame::AdminResult(AdminResult::refused("admin_keys", "", &refusal())))),
        Frame::UpdateOffer(o) => Some(Some(Frame::UpdateState(frames::UpdateState {
            transfer_id: Some(o.transfer_id.clone()),
            cmd_id: None,
            phase: UpdatePhase::Refused,
            version: o.version.clone(),
            from: Some(crate::PRODUCT_VERSION.into()),
            code: Some(NOT_NODE_OWNER.into()),
            message: Some(why.into()),
            at: node.now(),
            steps: Vec::new(),
        }))),
        Frame::UpdateChunk(_) => Some(None),
        _ => None,
    }
}

/// An assign the share may not start now stays queued on its server, with
/// the reason (the same shape as a node refusal).
fn gate_frame(node: &Node, job: &str, generation: u32, code: &str) -> Frame {
    let detail = match code {
        codes::SHARE_PAUSED => "the node owner paused this server's share",
        codes::SHARE_EXPIRED => "this server's share on the node has ended",
        codes::SHARE_REVOKED => "the node owner revoked this server",
        codes::TENANT_WALL_MISSING => "the node lost its job wall; it serves only its owner server",
        _ => "this server isn't approved on the node yet (confirm its SAS)",
    };
    Frame::State(frames::StateUpdate {
        queue: None,
        job_id: job.to_string(),
        generation,
        state: JobState::Queued,
        reason: Some(code.to_string()),
        detail: Some(detail.to_string()),
        at: node.now(),
        exit: None,
        src: None,
        log_seq: 0,
        outputs: None,
    })
}

/// Dial, attach, serve, back off — forever: the owner server (today's one
/// session; solo mode is just this), plus one session per other server
/// `servers.toml` approves ([`run_servers`]).
pub async fn run_forever(node: Arc<Node>) {
    let owner = node.owner();
    tokio::select! {
        _ = run_tenant_forever(node.clone(), owner) => {}
        _ = run_servers(node) => {}
    }
}

/// Sleep, or wake early when the node owner turns this server off.
async fn pause(serve: &mut tokio::sync::watch::Receiver<bool>, d: Duration) {
    tokio::select! {
        _ = tokio::time::sleep(d) => {}
        _ = async { let _ = serve.wait_for(|on| !*on).await; } => {}
    }
}

/// [`run_forever`] for one server. The owner's state is the status file's
/// (and it takes enrol requests); another server's is its [`Link`]
/// (`k2-node status` lists it). A non-owner must speak protocol 3.
///
/// [`Link`]: crate::node::tenant::Link
pub async fn run_tenant_forever(node: Arc<Node>, tenant: Arc<Tenant>) {
    let mut backoff = node.opts.backoff_min;
    let owner = tenant.is_owner();
    let status = |state: &str, route: Option<String>, err: Option<String>| {
        if owner {
            node.set_status_state(state, route, err);
        } else {
            let before = tenant.link.lock().unwrap().clone();
            if let Some(e) = err.as_ref().filter(|e| before.last_error.as_ref() != Some(*e) || before.state != state) {
                crate::nlog!("server slot {}: {state}: {e}", tenant.slot);
            }
            tenant.set_link(state, route, err, node.now());
        }
    };
    let cur = || if owner { node.status.lock().unwrap().state.clone() } else { tenant.link.lock().unwrap().state.clone() };
    let min_protocol = if owner { 0 } else { PROTOCOL_SHARED };
    let mut serve = tenant.serve.subscribe();
    loop {
        if !*serve.borrow_and_update() {
            status("stopped", None, None);
            if serve.wait_for(|on| *on).await.is_err() {
                return;
            }
            backoff = node.opts.backoff_min;
        }
        tenant.reload_pin();
        let pin = tenant.pin.lock().unwrap().clone();
        let pin = match pin {
            None => {
                if owner && crate::request::process(&node).await {
                    continue;
                }
                if cur() != "unenrolled" {
                    status("unenrolled", None, None);
                }
                pause(&mut serve, Duration::from_secs(5).min(node.opts.backoff_max)).await;
                continue;
            }
            Some(p) if p.revoked => {
                if owner && crate::request::process(&node).await {
                    continue;
                }
                status("revoked", None, p.revoked_reason.clone().map(|r| format!("Removed by the controller: {r}")));
                pause(&mut serve, Duration::from_secs(30).min(node.opts.backoff_max * 2)).await;
                continue;
            }
            Some(p) => p,
        };
        let boot = node.opts.clock.boot_id();
        let ledger_id = tenant.ledger_id();
        let mut last_err = None;
        for route in tenant.routes() {
            if cur() != "pending" {
                status("connecting", Some(route.clone()), last_err.clone());
            }
            let mut ws = match connect(&route, ATTACH_PATH).await {
                Ok(ws) => ws,
                Err(e) => {
                    last_err = Some(e);
                    continue;
                }
            };
            match attach_as(&mut ws, &node.key, &pin, &boot, &ledger_id, min_protocol).await {
                Ok(AttachOutcome::Session(start)) => {
                    status("online", Some(route.clone()), None);
                    if owner {
                        crate::nlog!("online via {route}");
                    } else {
                        crate::nlog!("server slot {}: online via {route} (protocol {})", tenant.slot, start.protocol);
                    }
                    let end = run_session(node.clone(), tenant.clone(), ws, start).await;
                    crate::nlog!("session ended: {end:?}");
                    backoff = node.opts.backoff_min;
                    last_err = Some(format!("{end:?}"));
                    if !matches!(end, SessionEnd::Revoked(_)) {
                        status("offline", None, last_err.clone());
                    }
                    break;
                }
                Ok(AttachOutcome::Refused(r)) => {
                    last_err = Some(format!("{}: {}", r.code, r.message));
                    match r.code.as_str() {
                        "pending_confirmation" => status("pending", Some(route.clone()), None),
                        "revoked" => node.revoke(&tenant, &r.message),
                        _ => status("offline", Some(route.clone()), last_err.clone()),
                    }
                    break;
                }
                Ok(AttachOutcome::TooOld(p)) => {
                    let _ = ws.close(None).await;
                    tenant.update_flags(|f| f.refused = Some(NODE_REFUSED_TOO_OLD.into()));
                    last_err = Some(format!("{NODE_REFUSED_TOO_OLD}: the server speaks protocol {p}; update it to 0.46.3"));
                    status("refused", Some(route.clone()), last_err.clone());
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    let _ = ws.close(None).await;
                }
            }
        }
        if let Some(e) = &last_err {
            let st = cur();
            if st != "pending" && st != "revoked" && st != "online" && st != "refused" {
                status("offline", None, Some(e.clone()));
            }
        }
        let wait = backoff + Duration::from_millis(crate::util::jitter_ms(backoff.as_millis() as u64 / 2 + 1));
        pause(&mut serve, wait).await;
        backoff = (backoff * 2).min(node.opts.backoff_max);
    }
}

const NODE_REFUSED_TOO_OLD: &str = codes::SERVER_TOO_OLD;

// ── many servers: the servers loop ───────────────────────────────────

/// How often the node asks the executor for due `servers.toml`
/// housekeeping (a confirmed add, an expired pending line, a due wipe).
const PRUNE_EVERY: Duration = Duration::from_secs(30);

/// How often the servers loop asks whether the per-tenant wall is on.
const WALL_PROBE_EVERY: Duration = Duration::from_secs(60);

/// Is this node's per-tenant wall really on (Linux tenant users ready for
/// every approved server)? Lane S3's `Node::tenant_wall_on()` answers; this
/// is the one call site to wire at the S3 integration
/// (`node.tenant_wall_on().await`). Until then: off, so no offer claims a
/// wall that isn't there.
async fn tenant_wall_on(node: &Arc<Node>) -> bool {
    node.tenant_wall_on().await
}

/// The servers loop's memory between passes.
#[derive(Default)]
struct ServersLoop {
    /// The last `servers.toml` that read and validated (§14: a broken file
    /// never fails open; the node serves on with this).
    last_good: Option<ServersFile>,
    /// Which line each open tenant serves: `(fp_prefix, added_at)`. A slot
    /// whose line changed hands is ended and reopened fresh.
    keys: HashMap<Slot, (String, i64)>,
    /// Each open tenant's `run_tenant_forever`.
    tasks: HashMap<Slot, tokio::task::JoinHandle<()>>,
    /// Enrol request ids already tried (once each).
    /// Enrol attempts per request id: how many failed, and when the next
    /// may run (backoff; a request that succeeded is never tried again).
    enroll_tries: HashMap<String, (u32, std::time::Instant)>,
    /// The last enrol failure per slot, shown in `k2-node servers`.
    enroll_error: std::collections::BTreeMap<Slot, String>,
    last_prune: Option<std::time::Instant>,
    /// When the per-tenant wall was last probed (lane S3's probe is an
    /// executor round trip: once a minute).
    last_wall_probe: Option<std::time::Instant>,
    prune_error: Option<String>,
    /// What the node queue was last told about each server's share.
    fq: std::collections::BTreeMap<Slot, crate::fairq::ShareInfo>,
    /// The SAS file as last written (slot → SAS).
    sas: Option<std::collections::BTreeMap<String, String>>,
}

impl Drop for ServersLoop {
    /// The loop is gone (shutdown): every other server's session ends on
    /// its own (its sub-tasks clean up), the idle tasks go with the runtime.
    fn drop(&mut self) {
        for (_, h) in self.tasks.drain() {
            drop(h);
        }
    }
}

/// Every `config_poll`: read `servers.toml`, open a tenant (and its
/// session) per server it approves, enrol a pending one, end the ones it
/// revoked or that expired, and show them all in the status file. No file
/// = solo mode: only the owner's session, as today.
pub async fn run_servers(node: Arc<Node>) {
    let mut st = ServersLoop::default();
    if node.opts.dev {
        node.status.lock().unwrap().dev = true;
        node.write_status();
    }
    loop {
        servers_pass(&node, &mut st).await;
        tokio::time::sleep(node.opts.config_poll).await;
    }
}

/// A session gets a freshly computed queue view at most this often.
const QUEUE_VIEW_EVERY: Duration = Duration::from_secs(1);

/// Which line a slot's node-only state belongs to (`state/tenants/<slot>/`).
const LINE_MARK_FILE: &str = "line";

fn line_mark(e: &ServerEntry) -> String {
    format!("{}:{}", e.fp_prefix, e.added_at)
}

/// Forget a slot that has no server: its state dir and pin (the node's own
/// files; quiet when already gone).
fn forget_slot(layout: &crate::paths::Layout, slot: Slot) {
    let t = layout.tenant(slot);
    let (state, pin) = (t.state(), t.pin());
    if state.exists() {
        match std::fs::remove_dir_all(&state) {
            Ok(()) => crate::nlog!("server slot {slot}: no server there any more; its node state is gone"),
            Err(e) => crate::nlog!("server slot {slot}: forget {}: {e}", state.display()),
        }
    }
    if let Err(e) = std::fs::remove_file(&pin) {
        if e.kind() != std::io::ErrorKind::NotFound {
            crate::nlog!("server slot {slot}: forget {}: {e}", pin.display());
        }
    }
}

fn line_key(e: &ServerEntry) -> (String, i64) {
    (e.fp_prefix.clone(), e.added_at)
}

/// Set `t`'s share view; wake its offer sender when it changed.
fn set_share(t: &Tenant, info: Option<ShareInfo>) -> bool {
    let mut g = t.share.lock().unwrap();
    if *g == info {
        return false;
    }
    *g = info;
    drop(g);
    t.offer_changed.notify_one();
    true
}

/// A line's status with what the node knows: a pending line whose pin is
/// confirmed counts as active (`servers.prune` writes it so later).
fn effective(layout: &crate::paths::Layout, e: &ServerEntry, now: i64) -> ServerStatus {
    let s = e.status(now);
    if e.slot == OWNER_SLOT || e.state != ShareState::Pending || e.expires_at.is_some_and(|x| now >= x) {
        return s;
    }
    match crate::identity::read_tenant_pin(&layout.tenant(e.slot).pin()) {
        Ok(Some((p, _))) if p.confirmed && !p.revoked && crate::node::tenant::pin_serves(e, &p) => ServerStatus::Active,
        _ => s,
    }
}

async fn servers_pass(node: &Arc<Node>, st: &mut ServersLoop) {
    let layout = node.layout().clone();
    let now = node.now();
    let owner = node.owner();
    let owner_fp = owner.pin.lock().unwrap().as_ref().filter(|p| !p.revoked).map(|p| p.controller_fp.clone());
    let mine = |f: &ServersFile| owner_fp.as_deref() == Some(f.owner.as_str());
    let (file, error) = match crate::admin::servers::read_file(&layout.config) {
        Ok(None) => {
            st.last_good = None;
            (None, None)
        }
        Ok(Some(f)) if mine(&f) => {
            st.last_good = Some(f.clone());
            (Some(f), None)
        }
        Ok(Some(_)) => (None, Some("servers.toml names another owner server than this node's; serving the owner only (re-add the servers after a re-enrol)".to_string())),
        Err(e) => (st.last_good.clone().filter(|f| mine(f)), Some(format!("{e} (serving on with the last good copy)"))),
    };

    // Each line's status as the node sees it, and the counts.
    let lines: Vec<(ServerEntry, ServerStatus)> = file.as_ref().map(|f| f.servers.iter().map(|e| (e.clone(), effective(&layout, e, now))).collect()).unwrap_or_default();
    let servers = lines.iter().filter(|(_, s)| matches!(s, ServerStatus::Active | ServerStatus::Paused)).count() as u32;    // Other servers see the owner by its own name when its line has only
    // an address for a label (enrolled over loopback: `127.0.0.1-60710`).
    let owner_label = file.as_ref().and_then(|f| f.entry(OWNER_SLOT)).map(|e| e.label.clone()).unwrap_or_default();
    let owner_label = match node.owner_name.lock().unwrap().clone() {
        Some(name) if address_label(&owner_label) => name,
        _ => owner_label,
    };
    let info = |e: &ServerEntry| ShareInfo { entry: e.clone(), servers, shared: servers >= 2, owner_label: owner_label.clone() };
    set_share(&owner, file.as_ref().and_then(|f| f.entry(OWNER_SLOT)).map(info));

    // Open tenants whose line went away or changed hands: end them; once
    // their session stopped and no job of theirs runs, forget them so the
    // slot opens fresh (new ledger and pin) for its next server.
    for t in node.tenants().into_iter().filter(|t| !t.is_owner()) {
        let line = lines.iter().find(|(e, _)| e.slot == t.slot).map(|(e, _)| e);
        // A tenant this loop didn't open (a restarted loop) is the line's
        // only when its pin serves the line AND its state was opened for
        // this very line (the line mark, fingerprint and add time): a
        // re-added server never inherits an old one's tenant (review B9).
        if let Some(e) = line {
            let marked = std::fs::read_to_string(layout.tenant(t.slot).state().join(LINE_MARK_FILE)).is_ok_and(|m| m == line_mark(e));
            if !st.keys.contains_key(&t.slot) && marked && t.pin.lock().unwrap().as_ref().is_some_and(|p| crate::node::tenant::pin_serves(e, p)) {
                st.keys.insert(t.slot, line_key(e));
            }
        }
        let same = line.is_some_and(|e| st.keys.get(&t.slot) == Some(&line_key(e)));
        if same {
            continue;
        }
        // The slot was wiped (and maybe re-added): the old server's jobs end.
        // Its share reads revoked from here on (no new work; its last offer
        // says so), and its session stays open only until its stopped jobs
        // ended, so it hears `cancelled/share_revoked` and never counts them
        // lost and retries them.
        let ended = t.share().map(|mut s| {
            s.entry.state = ShareState::Revoked;
            s
        });
        set_share(&t, ended);
        node.stop_tenant(t.slot, JobState::Cancelled, codes::SHARE_REVOKED);
        node.queue_cancel_tenant(&t, codes::SHARE_REVOKED);
        let running = node.running_by_tenant().contains_key(&t.slot);
        if *t.serve.borrow() != running {
            t.serve.send_replace(running);
        }
        let idle = !running && t.hub.connected.borrow().eq(&0);
        if idle && node.drop_tenant(t.slot) {
            st.keys.remove(&t.slot);
            if let Some(h) = st.tasks.remove(&t.slot) {
                h.abort();
            }
        }
    }

    // A slot with no line and nothing open: its node-only state (ledger,
    // journals, artifacts, pin) is the node's to forget (root's wipe
    // removes only the slot's root, review A M1). Only with a good file in
    // hand: a missing or broken one forgets nothing.
    if let Some(f) = file.as_ref() {
        for slot in 1..k2_node_proto::share::MAX_SLOTS {
            if f.entry(slot).is_none() && node.tenant(slot).is_none() {
                forget_slot(&layout, slot);
            }
        }
    }

    let stop = crate::admin::servers::read_stop(&layout.config).unwrap_or_default();
    let mut due_prune = false;
    for (e, eff) in lines.iter().filter(|(e, _)| e.slot != OWNER_SLOT) {
        let slot = e.slot;
        if node.tenant(slot).is_some_and(|_| st.keys.get(&slot) != Some(&line_key(e))) {
            continue; // the old server's tenant is still ending
        }
        let pin = crate::identity::read_tenant_pin(&layout.tenant(slot).pin()).ok().flatten().map(|(p, _)| p);
        let pin_ok = pin.as_ref().is_some_and(|p| crate::node::tenant::pin_serves(e, p));
        due_prune |= e.state == ShareState::Revoked || (e.state == ShareState::Pending && (*eff == ServerStatus::Active || now >= e.pending_until));
        if e.state == ShareState::Pending && *eff == ServerStatus::Pending && !pin_ok {
            enroll_pending(node, st, e, &owner_label).await;
        }
        let t = match node.tenant(slot) {
            Some(t) => t,
            None => {
                let pin_now = crate::identity::read_tenant_pin(&layout.tenant(slot).pin()).ok().flatten();
                if !pin_now.is_some_and(|(p, _)| crate::node::tenant::pin_serves(e, &p)) || e.state == ShareState::Revoked {
                    continue;
                }
                // The slot's node-only state belongs to one line: another
                // server's (a wiped slot reused while the node was off)
                // goes before this one opens (its ledger, journals).
                let tstate = layout.tenant(slot).state();
                let mark = line_mark(e);
                if tstate.exists() && std::fs::read_to_string(tstate.join(LINE_MARK_FILE)).ok().as_deref() != Some(mark.as_str()) {
                    if let Err(err) = std::fs::remove_dir_all(&tstate) {
                        crate::nlog!("server slot {slot}: can't clear another server's state: {err}");
                        continue;
                    }
                }
                let t = match node.add_tenant(slot).await {
                    Ok(t) => t,
                    Err(err) => {
                        crate::nlog!("server slot {slot}: can't open: {err}");
                        continue;
                    }
                };
                if let Err(err) = crate::util::atomic_write(&tstate.join(LINE_MARK_FILE), mark.as_bytes(), 0o600) {
                    crate::nlog!("server slot {slot}: line mark: {err}");
                }
                st.keys.insert(slot, line_key(e));
                st.tasks.insert(slot, tokio::spawn(run_tenant_forever(node.clone(), t.clone())));
                t
            }
        };
        t.reload_pin();
        set_share(&t, Some(info(e)));
        if e.state == ShareState::Revoked && stop.slots.contains(&slot) {
            node.stop_tenant(slot, JobState::Cancelled, codes::SHARE_REVOKED);
        }
        // Jobs this server still has here: running, stopping, or queued and
        // not yet cancelled. Their final states and receipts go out on its
        // session, so a revoke or an end date closes it only once they end
        // (the server would otherwise count them lost and retry them).
        let open_jobs = node.running_by_tenant().contains_key(&slot) || t.ledger.lock().unwrap().non_terminal().map(|v| !v.is_empty()).unwrap_or(false);
        let want = if e.state == ShareState::Revoked {
            // Revoked: no new work; stay only to report the jobs still
            // running (they finish, or stop with `--now`) (§4.3).
            open_jobs
        } else {
            match t.status_at(now) {
                Some(ServerStatus::Active | ServerStatus::Paused | ServerStatus::Pending | ServerStatus::Refused) => true,
                // Past its end date: no new work; stay only to report the
                // jobs still running (§4.3).
                Some(ServerStatus::Expired) => open_jobs && e.state != ShareState::Pending,
                // Revoked by the server itself (its pin): its session
                // already ended with `Revoked`.
                _ => false,
            }
        };
        if *t.serve.borrow() != want {
            t.serve.send_replace(want);
        }
    }

    // Is the per-tenant wall really on? Every tenant's offer says so
    // (`tenant-wall`, protocol 3 only).
    if st.last_wall_probe.is_none_or(|t| t.elapsed() >= WALL_PROBE_EVERY) {
        st.last_wall_probe = Some(std::time::Instant::now());
        let on = tenant_wall_on(node).await;
        // Secret-env (SV18): an executor restart may have changed whether
        // it relays a stdin pipe; ask again with the wall probe.
        let stdin_changed = node.refresh_exec_stdin().await;
        for t in node.tenants() {
            if t.tenant_wall.swap(on, std::sync::atomic::Ordering::SeqCst) != on || stdin_changed {
                t.offer_changed.notify_one();
            }
        }
    }

    if due_prune && st.last_prune.is_none_or(|t| t.elapsed() >= PRUNE_EVERY) {
        st.last_prune = Some(std::time::Instant::now());
        ask_prune(node, st).await;
    }

    // The node queue's view of every share (S2): the line's limits and its
    // status as the node sees it (time, a confirmed SAS, a server that left
    // or was refused). No line = no share (the owner then gets the queue's
    // policy-sized default; a missing server can't queue).
    let mut fq = std::collections::BTreeMap::new();
    for (e, eff) in &lines {
        let t = node.tenant(e.slot).filter(|_| e.slot == OWNER_SLOT || st.keys.get(&e.slot) == Some(&line_key(e)));
        let status = t.and_then(|t| t.status_at(now)).unwrap_or(*eff);
        fq.insert(e.slot, crate::fairq::ShareInfo { label: e.label.clone(), limits: e.limits(), status });
    }
    for slot in st.fq.keys().filter(|s| !fq.contains_key(s)).copied().collect::<Vec<_>>() {
        node.fairq.set_share(slot, None);
    }
    for (slot, info) in &fq {
        if st.fq.get(slot) != Some(info) {
            node.fairq.set_share(*slot, Some(info.clone()));
        }
    }
    st.fq = fq;

    // The status file lists the other servers.
    let running = node.running_by_tenant();
    let out: Vec<ServerLine> = lines
        .iter()
        .filter(|(e, _)| e.slot != OWNER_SLOT)
        .map(|(e, eff)| {
            let t = node.tenant(e.slot).filter(|_| st.keys.get(&e.slot) == Some(&line_key(e)));
            let pin = t.as_ref().and_then(|t| t.pin.lock().unwrap().clone());
            let link = t.as_ref().map(|t| t.link.lock().unwrap().clone()).unwrap_or_default();
            let state = t.as_ref().and_then(|t| t.status_at(now)).unwrap_or(*eff);
            ServerLine {
                slot: e.slot,
                label: e.label.clone(),
                state: state.as_str().into(),
                trust: e.trust.as_str().into(),
                link: if link.state.is_empty() {
                    if e.state == ShareState::Pending {
                        if st.enroll_error.contains_key(&e.slot) { "enrol_failed (retrying)".into() } else { "enrolling".into() }
                    } else {
                        "unenrolled".into()
                    }
                } else {
                    link.state
                },
                route: link.route,
                last_error: link.last_error.or_else(|| (e.state == ShareState::Pending).then(|| st.enroll_error.get(&e.slot).cloned()).flatten()),
                // The SAS isn't for everyone who can read the status file
                // (0644): it goes to the 0600 SAS file below (review B5).
                sas: None,
                fp: pin.as_ref().map(|p| p.controller_fp.clone()),
                running: running.get(&e.slot).map(|v| v.len() as u32).unwrap_or(0),
                expires_at: e.expires_at,
                skew_secs: link.skew_secs,
            }
        })
        .collect();
    // Pending servers' SAS, for the node owner at the machine only (0600).
    let sas: std::collections::BTreeMap<String, String> = lines
        .iter()
        .filter(|(e, _)| e.slot != OWNER_SLOT)
        .filter_map(|(e, _)| {
            let t = node.tenant(e.slot).filter(|_| st.keys.get(&e.slot) == Some(&line_key(e)));
            let pin = t.as_ref().and_then(|t| t.pin.lock().unwrap().clone()).or_else(|| crate::identity::read_tenant_pin(&layout.tenant(e.slot).pin()).ok().flatten().map(|(p, _)| p));
            pin.filter(|p| !p.confirmed && !p.revoked && crate::node::tenant::pin_serves(e, p)).map(|p| (e.slot.to_string(), p.sas))
        })
        .collect();
    if st.sas.as_ref() != Some(&sas) {
        let p = layout.run().join(crate::admin::servers::SAS_FILE);
        let r = if sas.is_empty() {
            match std::fs::remove_file(&p) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            }
        } else {
            serde_json::to_vec(&sas).map_err(|e| e.to_string()).and_then(|b| crate::util::atomic_write(&p, &b, 0o600))
        };
        match r {
            Ok(()) => st.sas = Some(sas),
            Err(e) => crate::nlog!("servers SAS file: {e}"),
        }
    }
    let error = error.or_else(|| st.prune_error.clone());
    let write = {
        let mut s = node.status.lock().unwrap();
        let changed = s.servers != out || s.servers_error != error;
        s.servers = out;
        s.servers_error = error;
        changed
    };
    if write {
        node.write_status();
    }
}

/// Run the pending add's enrol once (§4.1 step 4): the node enrols itself
/// with the new server using the code's binding, refusing a server whose
/// key doesn't start with the code's prefix; the SAS goes to the status
/// file and the result file for the node owner to read out.
async fn enroll_pending(node: &Arc<Node>, st: &mut ServersLoop, e: &ServerEntry, owner_label: &str) {
    use crate::admin::servers::{read_enroll, EnrollOutcome, ENROLL_RESULT_FILE};
    let layout = node.layout().clone();
    let req = match read_enroll(&layout.config) {
        Ok(Some(r)) => r,
        Ok(None) => return,
        Err(err) => {
            crate::nlog!("server enrol: {err}");
            return;
        }
    };
    let now = node.now();
    if req.slot != e.slot || req.fp_prefix != e.fp_prefix || now >= req.expires_at {
        return;
    }
    // A failed attempt is retried with backoff until the code's 10 minutes
    // end (the server may only need its old row removed first).
    if st.enroll_tries.get(&req.id).is_some_and(|(_, next)| std::time::Instant::now() < *next) {
        return;
    }
    let result_path = layout.run().join(ENROLL_RESULT_FILE);
    let last: Option<EnrollOutcome> = std::fs::read(&result_path).ok().and_then(|b| serde_json::from_slice(&b).ok());
    if last.is_some_and(|o| o.id == req.id && o.ok) {
        return;
    }
    let name = node.owner().pin.lock().unwrap().as_ref().map(|p| p.name.clone());
    let t = layout.tenant(e.slot);
    let r: Result<crate::enroll::Enrolled, String> = async {
        let name = name.ok_or("this node has no owner server")?;
        let cred = crate::enroll::Credential::Binding { binding: req.code_binding.clone(), fp_prefix: req.fp_prefix.clone() };
        let mut ws = connect(&req.url, ENROLL_PATH).await?;
        let shared_by = (!owner_label.is_empty()).then_some(owner_label);
        let done = crate::enroll::enroll_shared(&mut ws, &node.key, &req.url, &cred, &name, &Default::default(), now, shared_by).await?;
        t.ensure_state()?;
        crate::identity::write_tenant_pin(&t.pin(), &done.pin, &Default::default())?;
        Ok(done)
    }
    .await;
    let o = match r {
        Ok(done) => {
            crate::nlog!("server slot {}: enrolled with {} (pending): its SAS is in `k2-node servers` (never in the log)", e.slot, req.url);
            st.enroll_tries.remove(&req.id);
            st.enroll_error.remove(&e.slot);
            EnrollOutcome { id: req.id.clone(), slot: e.slot, ok: true, message: format!("enrolled with {} (pending): read code {} to its owner", e.label, done.sas), sas: Some(done.sas), at: now }
        }
        Err(err) => {
            let tries = st.enroll_tries.get(&req.id).map(|(n, _)| n + 1).unwrap_or(1);
            let wait = enroll_backoff(tries);
            crate::nlog!("server slot {}: enrol with {} failed (try {tries}, again in {}s): {err}", e.slot, req.url, wait.as_secs());
            st.enroll_tries.insert(req.id.clone(), (tries, std::time::Instant::now() + wait));
            st.enroll_error.insert(e.slot, err.clone());
            EnrollOutcome { id: req.id.clone(), slot: e.slot, ok: false, message: err, sas: None, at: now }
        }
    };
    if let Ok(b) = serde_json::to_vec_pretty(&o) {
        if let Err(err) = crate::util::atomic_write(&result_path, &b, 0o600) {
            crate::nlog!("server enrol result: {err}");
        }
    }
}

/// A server label that is only an address (`127.0.0.1-60710`,
/// `localhost-8080`, `::1`, `10.0.0.5`): `servers::label_of` of a URL
/// with no name in it.
fn address_label(label: &str) -> bool {
    let host = match label.rsplit_once('-') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => h,
        _ => label,
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    // `[::1]:80` comes out of label_of as `---1--80`.
    host.is_empty() || host.starts_with("--") || host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok()
}

#[cfg(test)]
mod address_label_tests {
    #[test]
    fn address_labels_are_addresses_only() {
        for a in ["127.0.0.1-60710", "localhost-8080", "10.0.0.5", "---1--80", "192.168.1.20"] {
            assert!(super::address_label(a), "{a}");
        }
        for n in ["rosson.k2.dev", "z3thon.k2.dev-8443", "mini-1", "k2n2-m1"] {
            assert!(!super::address_label(n), "{n}");
        }
    }
}

/// The wait before enrol attempt `tries + 1`: 5 s, 15 s, 30 s, then a
/// minute each (the code lasts 10 minutes).
fn enroll_backoff(tries: u32) -> Duration {
    Duration::from_secs(match tries {
        0 | 1 => 5,
        2 => 15,
        3 => 30,
        _ => 60,
    })
}

/// Ask the root executor for `servers.prune` (best effort; the node acts
/// on what it knows meanwhile).
async fn ask_prune(node: &Arc<Node>, st: &mut ServersLoop) {
    use crate::admin::exec::{LocalRequest, LocalResponse, ServersOp};
    let sock = node.opts.admin_socket.clone();
    let r = tokio::task::spawn_blocking(move || {
        crate::admin::server::request(&sock, &LocalRequest::Servers { servers: ServersOp::Prune }, None, Duration::from_secs(30))
    })
    .await;
    let err = match r {
        Ok(Ok(LocalResponse::Result(res))) if res.state == "done" => None,
        Ok(Ok(LocalResponse::Result(res))) => Some(format!("servers.prune: {}", res.message.unwrap_or(res.state))),
        Ok(Ok(_)) => Some("servers.prune: unexpected executor answer".to_string()),
        Ok(Err(e)) => Some(format!("servers.prune: {}: {}", e.code, e.message)),
        Err(e) => Some(format!("servers.prune: {e}")),
    };
    if err != st.prune_error {
        if let Some(e) = &err {
            crate::nlog!("{e}");
        }
        st.prune_error = err;
    }
}
