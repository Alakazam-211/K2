//! A loopback fake controller built only from k2-node-proto, plus helpers
//! to start a node against it in temp dirs. No real network, no real
//! users, no system paths.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use k2_node::config::MachineFacts;
use k2_node::identity::Pin;
use k2_node::node::{Node, NodeOptions};
use k2_node::paths::Layout;
use k2_node_proto::crypto::{self, SigningKey};
use k2_node_proto::frames::{
    self, Assign, Challenge, Direction, EnrollChallenge, EnrollDone, Fence, Frame, HandshakeFrame, JobLimits, Plan,
    Refused, ResumeHint, Welcome,
};
use k2_node_proto::pairing;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone, Debug, PartialEq)]
pub enum NodeState {
    Pending,
    Active,
    Revoked,
}

pub struct Registered {
    pub spki: Vec<u8>,
    pub state: NodeState,
    pub node_id: String,
    pub name: String,
}

pub enum Out {
    Frame(Frame),
    /// Raw text (forgeries, replays).
    Raw(String),
    Close,
}

/// One accepted attach session as the test sees it.
pub struct Conn {
    pub rx: mpsc::UnboundedReceiver<Frame>,
    pub tx: mpsc::UnboundedSender<Out>,
    /// Every raw text frame the controller sent, in order (for replays).
    pub sent_raw: Arc<Mutex<Vec<String>>>,
    pub closed: Arc<tokio::sync::Notify>,
    pub is_closed: Arc<std::sync::atomic::AtomicBool>,
}

impl Conn {
    pub async fn next(&mut self) -> Frame {
        tokio::time::timeout(Duration::from_secs(20), self.rx.recv())
            .await
            .expect("timed out waiting for a frame from the node")
            .expect("node session ended")
    }

    /// Frames until `pred` matches (returns all seen, last = the match).
    pub async fn until(&mut self, mut pred: impl FnMut(&Frame) -> bool) -> Vec<Frame> {
        let mut seen = Vec::new();
        loop {
            let f = self.next().await;
            let hit = pred(&f);
            seen.push(f);
            if hit {
                return seen;
            }
        }
    }

    pub fn send(&self, f: Frame) {
        self.tx.send(Out::Frame(f)).expect("conn writer gone");
    }

    pub async fn wait_closed(&self, within: Duration) -> bool {
        if self.is_closed.load(std::sync::atomic::Ordering::SeqCst) {
            return true;
        }
        tokio::time::timeout(within, self.closed.notified()).await.is_ok()
            || self.is_closed.load(std::sync::atomic::Ordering::SeqCst)
    }
}

pub struct Fake {
    pub key: Arc<SigningKey>,
    pub base: String,
    pub nodes: Arc<Mutex<HashMap<String, Registered>>>,
    /// Normalized codes accepted for enrollment.
    pub codes: Arc<Mutex<Vec<String>>>,
    pub resume: Arc<Mutex<Vec<ResumeHint>>>,
    pub conns: mpsc::UnboundedReceiver<Conn>,
    pub attaches: Arc<std::sync::atomic::AtomicU64>,
    /// SAS the fake computed on its side for the last enrollment.
    pub last_sas: Arc<Mutex<Option<String>>>,
}

impl Fake {
    pub async fn start() -> Self {
        let key = Arc::new(SigningKey::generate().unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let nodes: Arc<Mutex<HashMap<String, Registered>>> = Arc::new(Mutex::new(HashMap::new()));
        let codes = Arc::new(Mutex::new(Vec::new()));
        let resume = Arc::new(Mutex::new(Vec::new()));
        let attaches = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let last_sas = Arc::new(Mutex::new(None));
        let (ctx, crx) = mpsc::unbounded_channel();
        let (k, n, c, r, a, s) = (key.clone(), nodes.clone(), codes.clone(), resume.clone(), attaches.clone(), last_sas.clone());
        tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else { return };
                let (k, n, c, r, a, s, ctx) = (k.clone(), n.clone(), c.clone(), r.clone(), a.clone(), s.clone(), ctx.clone());
                tokio::spawn(async move {
                    let path = Arc::new(Mutex::new(String::new()));
                    let p2 = path.clone();
                    #[allow(clippy::result_large_err)]
                    let cb = move |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
                                   resp: tokio_tungstenite::tungstenite::handshake::server::Response| {
                        *p2.lock().unwrap() = req.uri().path().to_string();
                        Ok(resp)
                    };
                    let Ok(ws) = tokio_tungstenite::accept_hdr_async(tcp, cb).await else { return };
                    let p = path.lock().unwrap().clone();
                    if p == "/cli/compute/enroll" {
                        serve_enroll(ws, &k, &n, &c, &s).await;
                    } else if p == "/cli/compute/attach" {
                        a.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        serve_attach(ws, k, n, r, ctx).await;
                    }
                });
            }
        });
        Self { key, base, nodes, codes, resume, conns: crx, attaches, last_sas }
    }

    pub async fn next_conn(&mut self) -> Conn {
        tokio::time::timeout(Duration::from_secs(20), self.conns.recv()).await.expect("no attach within 20 s").expect("fake gone")
    }

    pub fn register_active(&self, node_key: &SigningKey, node_id: &str, name: &str) {
        self.nodes.lock().unwrap().insert(
            node_key.fingerprint(),
            Registered { spki: node_key.spki_der().to_vec(), state: NodeState::Active, node_id: node_id.into(), name: name.into() },
        );
    }

    pub fn pin_for(&self, node_id: &str, name: &str) -> Pin {
        Pin {
            controller_fp: self.key.fingerprint(),
            controller_public_key_pem: self.key.spki_pem(),
            routes: vec![self.base.clone()],
            node_id: node_id.into(),
            name: name.into(),
            enrolled_at: 1,
            sas: "000 000".into(),
            revoked: false,
            revoked_reason: None,
            labels: BTreeMap::new(),
            confirmed: true,
        }
    }
}

async fn send_hs<S>(ws: &mut S, f: &HandshakeFrame)
where
    S: futures_util::Sink<Message> + Unpin,
{
    let _ = ws.send(Message::Text(serde_json::to_string(f).unwrap())).await;
}

async fn recv_hs<S>(ws: &mut S) -> Option<HandshakeFrame>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match ws.next().await? {
            Ok(Message::Text(t)) => return serde_json::from_str(&t).ok(),
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => continue,
            _ => return None,
        }
    }
}

async fn serve_enroll<S>(mut ws: S, key: &SigningKey, nodes: &Mutex<HashMap<String, Registered>>, codes: &Mutex<Vec<String>>, last_sas: &Mutex<Option<String>>)
where
    S: futures_util::Sink<Message> + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let Some(HandshakeFrame::EnrollRequest(req)) = recv_hs(&mut ws).await else { return };
    let code = codes.lock().unwrap().iter().find(|c| pairing::code_binding(c) == req.code_binding).cloned();
    let Some(code) = code else {
        send_hs(&mut ws, &HandshakeFrame::Refused(Refused { code: "code_invalid".into(), message: "no such code".into() })).await;
        return;
    };
    let node_fp = crypto::fingerprint_of_spki_pem(&req.node_public_key_pem).unwrap();
    let nonce = crypto::b64(&crypto::random_bytes(16).unwrap());
    let binding = pairing::code_binding(&code);
    let cfp = key.fingerprint();
    let sig = key
        .sign_domain(frames::DOMAIN_ENROLL_CONTROLLER, &[node_fp.as_bytes(), nonce.as_bytes(), binding.as_bytes(), cfp.as_bytes()])
        .unwrap();
    let node_id = format!("node-{}", &node_fp[..8]);
    send_hs(&mut ws, &HandshakeFrame::EnrollChallenge(EnrollChallenge {
        controller_public_key_pem: key.spki_pem(),
        enroll_nonce: nonce.clone(),
        node_id: node_id.clone(),
        sig,
    }))
    .await;
    let Some(HandshakeFrame::EnrollProof(p)) = recv_hs(&mut ws).await else { return };
    let spki = crypto::pem_decode("PUBLIC KEY", &req.node_public_key_pem).unwrap();
    if !crypto::verify_domain(&spki, frames::DOMAIN_ENROLL_NODE, &[cfp.as_bytes(), nonce.as_bytes(), binding.as_bytes(), node_fp.as_bytes()], &p.sig) {
        send_hs(&mut ws, &HandshakeFrame::Refused(Refused { code: "bad_proof".into(), message: "x".into() })).await;
        return;
    }
    codes.lock().unwrap().retain(|c| c != &code);
    *last_sas.lock().unwrap() = Some(pairing::display_sas(&pairing::compute_sas(&cfp, &node_fp, &nonce, &binding)));
    nodes.lock().unwrap().insert(node_fp, Registered { spki, state: NodeState::Pending, node_id: node_id.clone(), name: req.name.clone() });
    send_hs(&mut ws, &HandshakeFrame::EnrollDone(EnrollDone { node_id, name: req.name, state: "pending".into() })).await;
}

async fn serve_attach<S>(
    mut ws: S,
    key: Arc<SigningKey>,
    nodes: Arc<Mutex<HashMap<String, Registered>>>,
    resume: Arc<Mutex<Vec<ResumeHint>>>,
    conns: mpsc::UnboundedSender<Conn>,
) where
    S: futures_util::Sink<Message> + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin + Send + 'static,
{
    let Some(HandshakeFrame::Hello(h)) = recv_hs(&mut ws).await else { return };
    let found = nodes.lock().unwrap().get(&h.node_fp).map(|r| (r.spki.clone(), r.state.clone(), r.node_id.clone(), r.name.clone()));
    let Some((spki, state, node_id, name)) = found else { return };
    let cfp = key.fingerprint();
    let nonce_c = crypto::nonce_b64().unwrap();
    let sig = key.sign_domain(frames::DOMAIN_ATTACH_CONTROLLER, &[h.nonce_n.as_bytes(), h.node_fp.as_bytes(), nonce_c.as_bytes()]).unwrap();
    send_hs(&mut ws, &HandshakeFrame::Challenge(Challenge { controller_fp: cfp.clone(), nonce_c: nonce_c.clone(), sig, protocol: frames::PROTOCOL })).await;
    let Some(HandshakeFrame::Proof(p)) = recv_hs(&mut ws).await else { return };
    if !crypto::verify_domain(&spki, frames::DOMAIN_ATTACH_NODE, &[nonce_c.as_bytes(), cfp.as_bytes(), h.nonce_n.as_bytes()], &p.sig) {
        return;
    }
    match state {
        NodeState::Pending => {
            send_hs(&mut ws, &HandshakeFrame::Refused(Refused { code: "pending_confirmation".into(), message: "confirm the SAS".into() })).await;
            return;
        }
        NodeState::Revoked => {
            send_hs(&mut ws, &HandshakeFrame::Refused(Refused { code: "revoked".into(), message: "removed".into() })).await;
            return;
        }
        NodeState::Active => {}
    }
    let session = frames::session_id(&h.nonce_n, &nonce_c);
    let welcome = Frame::Welcome(Welcome {
        node_id,
        name,
        controller: "fake.k2.dev".into(),
        resume: resume.lock().unwrap().clone(),
        routes: vec![],
        narrow: None,
        server_time: 0,
    });
    let first = frames::seal(&key, Direction::ControllerToNode, &session, 0, &welcome).unwrap();
    let _ = ws.send(Message::Text(first.clone())).await;
    let (in_tx, in_rx) = mpsc::unbounded_channel();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Out>();
    let sent_raw = Arc::new(Mutex::new(vec![first]));
    let closed = Arc::new(tokio::sync::Notify::new());
    let is_closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _ = conns.send(Conn { rx: in_rx, tx: out_tx, sent_raw: sent_raw.clone(), closed: closed.clone(), is_closed: is_closed.clone() });
    let (mut sink, mut stream) = ws.split();
    let mut seq = 1u64;
    let mut expected = 0u64;
    loop {
        tokio::select! {
            o = out_rx.recv() => match o {
                None | Some(Out::Close) => { let _ = sink.close().await; break; }
                Some(Out::Raw(t)) => { if sink.send(Message::Text(t)).await.is_err() { break; } }
                Some(Out::Frame(f)) => {
                    let t = frames::seal(&key, Direction::ControllerToNode, &session, seq, &f).unwrap();
                    seq += 1;
                    sent_raw.lock().unwrap().push(t.clone());
                    if sink.send(Message::Text(t)).await.is_err() { break; }
                }
            },
            m = stream.next() => match m {
                Some(Ok(Message::Text(t))) => {
                    match frames::open(&spki, Direction::NodeToController, &session, expected, &t) {
                        Ok(f) => { expected += 1; let _ = in_tx.send(f); }
                        Err(e) => panic!("fake controller: bad frame from node: {e}"),
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            }
        }
    }
    is_closed.store(true, std::sync::atomic::Ordering::SeqCst);
    closed.notify_waiters();
}

// ── node setup ───────────────────────────────────────────────────────

pub struct TestNode {
    pub node: Arc<Node>,
    pub layout: Layout,
    pub runner: tokio::task::JoinHandle<()>,
    pub dir: PathBuf,
}

impl Drop for TestNode {
    fn drop(&mut self) {
        self.runner.abort();
        self.node.stop_all(k2_node_proto::frames::JobState::Cancelled, "test_over");
    }
}

pub fn facts() -> MachineFacts {
    MachineFacts { cores: 8, mem_bytes: 32 << 30, disk_total_bytes: 1 << 40, has_battery: false, is_macos: cfg!(target_os = "macos") }
}

pub fn write_policy(layout: &Layout, extra: &str) {
    std::fs::create_dir_all(&layout.config).unwrap();
    let body = format!("availability = \"always\"\nforeign_locks = []\nwrite_smoke_lock = \"\"\ndisk_floor_gb = 0\n{extra}");
    std::fs::write(layout.policy(), body).unwrap();
}

pub fn options(layout: Layout) -> NodeOptions {
    let mut o = NodeOptions::new(layout);
    o.dev = true;
    o.cancel_grace = Duration::from_millis(500);
    o.config_poll = Duration::from_millis(100);
    o.backoff_min = Duration::from_millis(50);
    o.backoff_max = Duration::from_millis(300);
    o.keep_awake = false;
    o.use_cgroups = false;
    o.facts = Some(facts());
    o
}

/// A node already enrolled and active at `fake`.
pub async fn start_node(fake: &Fake, tag: &str, policy_extra: &str, tweak: impl FnOnce(&mut NodeOptions)) -> TestNode {
    let dir = k2_node::util::temp_dir(tag);
    let layout = Layout::new(dir.join("home"), dir.join("config"));
    layout.ensure().unwrap();
    write_policy(&layout, policy_extra);
    let key = k2_node::identity::load_or_create_key(&layout.key()).unwrap();
    fake.register_active(&key, "node-1", "mini-1");
    k2_node::identity::write_pin(&layout.pin(), &fake.pin_for("node-1", "mini-1")).unwrap();
    let mut o = options(layout.clone());
    tweak(&mut o);
    let node = Node::open(o).await.unwrap();
    node.spawn_background();
    let runner = tokio::spawn(k2_node::session::run_forever(node.clone()));
    TestNode { node, layout, runner, dir }
}

pub fn plan(job: &str, argv: &[&str]) -> Plan {
    Plan {
        job_id: job.into(),
        node_id: "node-1".into(),
        workspace_id: "ws-1".into(),
        workspace_label: "k2".into(),
        requested_by: "agent:k2".into(),
        argv: argv.iter().map(|s| s.to_string()).collect(),
        env: BTreeMap::new(),
        cwd: None,
        limits: JobLimits { max_secs: 60, cpu_millis: None, mem_bytes: None, disk_bytes: 0, log_cap_bytes: 1 << 20 },
        src: None,
        exclusive: false,
        created_at: 1,
    }
}

pub fn assign(p: Plan, generation: u32) -> Assign {
    Assign {
        fence: Fence { job_id: p.job_id.clone(), attempt: generation, generation, plan_digest: p.digest().unwrap() },
        plan: p,
    }
}

/// Collect a job's frames until its receipt.
pub async fn run_to_receipt(c: &mut Conn, job: &str) -> Vec<Frame> {
    c.until(|f| matches!(f, Frame::Receipt(r) if r.job_id == job)).await
}

pub fn logs_of(frames: &[Frame], job: &str) -> Vec<(u64, k2_node_proto::frames::LogStream, Vec<u8>)> {
    frames
        .iter()
        .filter_map(|f| match f {
            Frame::Log(l) if l.job_id == job => Some((l.seq, l.stream, crypto::unb64(&l.data).unwrap())),
            _ => None,
        })
        .collect()
}

pub fn states_of(frames: &[Frame], job: &str) -> Vec<k2_node_proto::frames::StateUpdate> {
    frames
        .iter()
        .filter_map(|f| match f {
            Frame::State(s) if s.job_id == job => Some(s.clone()),
            _ => None,
        })
        .collect()
}

pub fn text(logs: &[(u64, k2_node_proto::frames::LogStream, Vec<u8>)], stream: k2_node_proto::frames::LogStream) -> String {
    logs.iter().filter(|l| l.1 == stream).map(|l| String::from_utf8_lossy(&l.2).into_owned()).collect()
}

/// Wait for the node's first offer after a fresh session.
pub async fn ready(c: &mut Conn) {
    c.until(|f| matches!(f, Frame::Offer(_))).await;
}

pub fn exists(p: &Path) -> bool {
    p.exists()
}
