//! Dialing the controller, the signed attach handshake (§7.3), one
//! session's loops, and the run loop with backoff (§11.5, §12.1).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use k2_node_proto::crypto::{self, SigningKey};
use k2_node_proto::frames::{
    self, Challenge, Direction, Frame, HandshakeFrame, Hello, JobState, JobsSnapshot, Proof, ReceiptFrame, Refused,
    Welcome, HANDSHAKE_SECS, MAX_FRAME_BYTES, PROTOCOL,
};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;

use crate::identity::Pin;
use crate::journal::JournalReader;
use crate::node::{journal_path, Node, NODE_VERSION};

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
}

pub enum AttachOutcome {
    Session(SessionStart),
    Refused(Refused),
}

/// The node side of the attach handshake. Every check fails closed.
pub async fn attach<S>(ws: &mut S, key: &SigningKey, pin: &Pin, boot_id: &str, ledger_id: &str) -> Result<AttachOutcome, String>
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
            Ok(Frame::Welcome(welcome)) => Ok(AttachOutcome::Session(SessionStart { session, welcome, controller_spki })),
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
}

type Tracks = Arc<Mutex<HashMap<(String, u32), Track>>>;

fn track(node: &Node, job: &str, gen: u32, since: u64, hinted: bool) -> Track {
    let mut reader = JournalReader::new(journal_path(node.layout(), job, gen));
    reader.skip_to(since);
    Track { reader, sent: None, hinted }
}

/// One pass of the shipper: logs first, then state, then the receipt.
/// `Err` when the session's queue is gone.
async fn ship_pass(node: &Node, tracks: &Tracks, tx: &mpsc::Sender<Frame>) -> Result<(), ()> {
    let keys: Vec<(String, u32)> = tracks.lock().unwrap().keys().cloned().collect();
    for key in keys {
        let Some(mut t) = tracks.lock().unwrap().remove(&key) else { continue };
        let row = node.ledger.lock().unwrap().get(&key.0, key.1).ok().flatten();
        let Some(row) = row else {
            if t.hinted {
                let f = Frame::State(frames::StateUpdate {
                    job_id: key.0.clone(),
                    generation: key.1,
                    state: JobState::Unknown,
                    reason: Some("not_on_node".into()),
                    detail: None,
                    at: node.now(),
                    exit: None,
                    src: None,
                    log_seq: 0,
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
            if t.reader.last_seq < row.log_seq {
                tracks.lock().unwrap().insert(key, t);
                continue;
            }
            if t.sent.as_ref() != Some(&state_key) {
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
                tx.send(node.row_state_frame(&row)).await.map_err(|_| ())?;
                t.sent = Some(state_key);
            }
            tracks.lock().unwrap().insert(key, t);
        }
    }
    Ok(())
}

/// Run one established session until it ends.
pub async fn run_session(node: Arc<Node>, ws: Ws, start: SessionStart) -> SessionEnd {
    run_session_on(node, ws, start).await
}

/// [`run_session`] over any WebSocket (tests use the loopback one).
pub async fn run_session_on<S>(node: Arc<Node>, ws: S, start: SessionStart) -> SessionEnd
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

    // Learned routes, the confirmed flag, narrowing.
    {
        let mut lr = node.learned_routes.lock().unwrap();
        for r in &start.welcome.routes {
            if crate::url::ws_url(r, ATTACH_PATH).is_ok() && !lr.contains(r) {
                lr.push(r.clone());
            }
        }
    }
    node.mark_confirmed();
    if let Some(n) = start.welcome.narrow.clone() {
        node.narrow(n);
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

    let epoch = node.hub.attach(tx.clone());

    // Tracks: resume hints + every open attempt.
    let tracks: Tracks = Arc::new(Mutex::new(HashMap::new()));
    {
        let mut t = tracks.lock().unwrap();
        for h in &start.welcome.resume {
            t.insert((h.job_id.clone(), h.generation), track(&node, &h.job_id, h.generation, h.since_seq, true));
        }
        for r in node.ledger.lock().unwrap().non_terminal().unwrap_or_default() {
            t.entry((r.job_id.clone(), r.generation)).or_insert_with(|| track(&node, &r.job_id, r.generation, 0, false));
        }
    }

    // Snapshot pages, then the first offer.
    let rows = node.ledger.lock().unwrap().all().unwrap_or_default();
    let jobs: Vec<_> = rows.iter().map(|r| r.to_ledger_job()).collect();
    let pages: Vec<_> = if jobs.is_empty() { vec![vec![]] } else { jobs.chunks(200).map(|c| c.to_vec()).collect() };
    let last = pages.len() - 1;
    for (i, p) in pages.into_iter().enumerate() {
        if tx.send(Frame::JobsSnapshot(JobsSnapshot { jobs: p, last: i == last })).await.is_err() {
            break;
        }
    }
    let first = node.offer();
    let _ = tx.send(Frame::Offer(first.clone())).await;

    // Shipper.
    let ship_node = node.clone();
    let ship_tracks = tracks.clone();
    let ship_tx = tx.clone();
    let shipper = tokio::spawn(async move {
        loop {
            if ship_pass(&ship_node, &ship_tracks, &ship_tx).await.is_err() {
                return;
            }
            tokio::select! {
                _ = ship_node.ship.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            }
        }
    });

    // Offers: on change, and every OFFER_SECS regardless.
    let offer_node = node.clone();
    let offer_tx = tx.clone();
    let offers = tokio::spawn(async move {
        let mut last_rev = first.revision;
        let mut tick = tokio::time::interval(offer_node.opts.offer_every);
        tick.tick().await;
        loop {
            let force = tokio::select! {
                _ = offer_node.offer_changed.notified() => false,
                _ = tick.tick() => true,
            };
            let o = offer_node.offer();
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
    let end = loop {
        let msg = tokio::select! {
            m = stream.next() => m,
            _ = async { while !writer.is_finished() { tokio::time::sleep(Duration::from_millis(200)).await; } } => {
                let why = writer_why.lock().unwrap().clone().unwrap_or_else(|| "stopped".into());
                break SessionEnd::Closed(format!("writer ended: {why}"));
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
        match frame {
            Frame::Assign(a) => {
                if let Some((job, gen)) = node.on_assign(a) {
                    tracks.lock().unwrap().insert((job.clone(), gen), track(&node, &job, gen, 0, false));
                    node.ship.notify_one();
                }
            }
            Frame::Transfer(t) => node.hub.route_transfer((t.job_id.clone(), t.generation, crate::node::kind_name(t.kind)), crate::node::TransferEvent::Chunk(t)),
            Frame::TransferFailed(t) => node.hub.route_transfer((t.job_id.clone(), t.generation, crate::node::kind_name(t.kind)), crate::node::TransferEvent::Failed(t)),
            Frame::Cancel(c) => node.cancel(&c.job_id, c.generation, &c.reason),
            Frame::PolicyNarrow(p) => node.narrow(p.limits),
            Frame::Revoked(r) => {
                node.revoke(&r.reason);
                break SessionEnd::Revoked(r.reason);
            }
            Frame::Welcome(_) => break SessionEnd::ProtocolError("second welcome".into()),
            other => break SessionEnd::ProtocolError(format!("{} can't come from the controller", other.kind())),
        }
    };
    node.hub.detach(epoch);
    shipper.abort();
    offers.abort();
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

/// Dial, attach, serve, back off — forever.
pub async fn run_forever(node: Arc<Node>) {
    let mut backoff = node.opts.backoff_min;
    loop {
        node.reload_pin();
        let pin = node.pin.lock().unwrap().clone();
        let pin = match pin {
            None => {
                node.set_status_state("unenrolled", None, None);
                tokio::time::sleep(Duration::from_secs(5).min(node.opts.backoff_max)).await;
                continue;
            }
            Some(p) if p.revoked => {
                node.set_status_state("revoked", None, p.revoked_reason.clone().map(|r| format!("Removed by the controller: {r}")));
                tokio::time::sleep(Duration::from_secs(30).min(node.opts.backoff_max * 2)).await;
                continue;
            }
            Some(p) => p,
        };
        let boot = node.opts.clock.boot_id();
        let ledger_id = node.ledger.lock().unwrap().ledger_id.clone();
        let mut last_err = None;
        for route in node.routes() {
            if node.status.lock().unwrap().state != "pending" {
                node.set_status_state("connecting", Some(route.clone()), last_err.clone());
            }
            let mut ws = match connect(&route, ATTACH_PATH).await {
                Ok(ws) => ws,
                Err(e) => {
                    last_err = Some(e);
                    continue;
                }
            };
            match attach(&mut ws, &node.key, &pin, &boot, &ledger_id).await {
                Ok(AttachOutcome::Session(start)) => {
                    node.set_status_state("online", Some(route.clone()), None);
                    crate::nlog!("online via {route}");
                    let end = run_session(node.clone(), ws, start).await;
                    crate::nlog!("session ended: {end:?}");
                    backoff = node.opts.backoff_min;
                    last_err = Some(format!("{end:?}"));
                    if !matches!(end, SessionEnd::Revoked(_)) {
                        node.set_status_state("offline", None, last_err.clone());
                    }
                    break;
                }
                Ok(AttachOutcome::Refused(r)) => {
                    last_err = Some(format!("{}: {}", r.code, r.message));
                    match r.code.as_str() {
                        "pending_confirmation" => node.set_status_state("pending", Some(route.clone()), None),
                        "revoked" => node.revoke(&r.message),
                        _ => node.set_status_state("offline", Some(route.clone()), last_err.clone()),
                    }
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    let _ = ws.close(None).await;
                }
            }
        }
        if let Some(e) = &last_err {
            let st = node.status.lock().unwrap().state.clone();
            if st != "pending" && st != "revoked" && st != "online" {
                node.set_status_state("offline", None, Some(e.clone()));
            }
        }
        let wait = backoff + Duration::from_millis(crate::util::jitter_ms(backoff.as_millis() as u64 / 2 + 1));
        tokio::time::sleep(wait).await;
        backoff = (backoff * 2).min(node.opts.backoff_max);
    }
}
