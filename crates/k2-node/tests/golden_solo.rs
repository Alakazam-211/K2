//! Solo-node golden frames (prd-compute-shared-queue-v1 §10.1, V66, C1).
//!
//! A node with one server must send what 0.46.2 sent. This test drives a
//! real node through a fixed script against a loopback controller that
//! speaks protocol 2 (a 0.46.2 server), with fixed keys and a fixed clock,
//! and compares every frame the node sends with
//! `tests/golden/solo-frames-v0.46.2.json`, recorded by this same file
//! built against the `v0.46.2` tag's `k2-node` and `k2-node-proto`.
//!
//! Masked, and nothing else:
//! - the four version fields V66 lets change: `Hello.protocol`,
//!   `Hello.node_version`, `Offer.protocol`, `Offer.node_version`;
//! - what can't be fixed: the hello nonce, the random ledger id, ECDSA
//!   signatures, live free memory and disk, this machine's tool versions
//!   and os/arch/vm labels, and the `system-packages` feature (present
//!   only where a package installer is);
//! - offer revisions after the first session (they count offers sent);
//! - the offer features added since 0.46.2 ([`BASE_0463_FEATURES`]:
//!   two from the 0.46.3 base before sharing, and 0.46.4's owner-only
//!   `wall-restart`; checked present, then removed).
//!
//! Re-record (only ever from the v0.46.2 sources, never from a newer tree):
//! `GOLDEN_RECORD=/abs/path/solo-frames-v0.46.2.json cargo test -p k2-node --test golden_solo`.
// pending k2-node Windows port: k2-node (the compute node runtime) is Unix-only
// until its Windows port (Phase 3: Job Objects instead of uids and cgroups).
#![cfg(unix)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use k2_node::identity::Pin;
use k2_node::ledger::Clock;
use k2_node::node::{Node, NodeOptions};
use k2_node::paths::Layout;
use k2_node_proto::crypto::SigningKey;
use k2_node_proto::frames::{
    self, Assign, Cancel, Challenge, Direction, Fence, Frame, HandshakeFrame, Hello, JobLimits, JobState, Plan, ResumeHint,
    Welcome,
};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// Fixed P-256 keys (test-only; generated once for this file).
const NODE_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgR1fDkvEdKXOCZW9z
MenGc9D4VTu5hk0S7iagrYc4A4ehRANCAARRzqh0EWQPE2yItBdiKASbZ3S6GjBM
E5sm0hzHtCKu2kbnUl/7V8JsWQHywMRjjK/0v2Iv1qV7AxgeFWZviO3G
-----END PRIVATE KEY-----
";
const CONTROLLER_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgXOPzvmDXBTvr3qBy
8luPWSWuco3MyGxCtyVsivieyqKhRANCAASMzOW0F7H10Ue3XndttRTqSNOCNqPl
XgKg63xc/2j9DFqVKNiTCk7VqOSbZHomEMjCmI7ArjpJMeHgEhlVqAD5
-----END PRIVATE KEY-----
";
const NOW: i64 = 1_760_000_000;
const BOOT: &str = "golden-boot";
/// What a 0.46.2 server says in `Challenge.protocol`.
const SERVER_PROTOCOL: u32 = 2;
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/solo-frames-v0.46.2.json");

/// Offer features added since 0.46.2: the 0.46.3 base before node sharing
/// (compute B / DX A: kept artifacts and the stall timeout), and 0.46.4's
/// `wall-restart` (owner-only; the controller branches on it for the
/// held wall plan). An old server ignores unknown features. Sharing must
/// add nothing here for a protocol-2 server: `servers`, `queue` and
/// `tenant-wall` go only to protocol ≥ 3.
const BASE_0463_FEATURES: &[&str] = &["artifacts", "stall", k2_node_proto::admin::FEATURE_WALL_RESTART];

fn recording() -> bool {
    std::env::var_os("GOLDEN_RECORD").is_some()
}

struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> i64 {
        NOW
    }
    fn boot_id(&self) -> String {
        BOOT.into()
    }
}

// ── a loopback 0.46.2-style server with a fixed key ──────────────────

struct Session {
    hello: Hello,
    rx: mpsc::UnboundedReceiver<Frame>,
    tx: mpsc::UnboundedSender<Option<Frame>>,
}

impl Session {
    async fn next(&mut self) -> Frame {
        tokio::time::timeout(Duration::from_secs(20), self.rx.recv())
            .await
            .expect("timed out waiting for a frame from the node")
            .expect("node session ended")
    }

    async fn until(&mut self, mut pred: impl FnMut(&Frame) -> bool) -> Vec<Frame> {
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

    fn send(&self, f: Frame) {
        self.tx.send(Some(f)).expect("session writer gone");
    }

    fn close(&self) {
        let _ = self.tx.send(None);
    }
}

struct Server {
    base: String,
    resume: Arc<Mutex<Vec<ResumeHint>>>,
    sessions: mpsc::UnboundedReceiver<Session>,
}

impl Server {
    async fn next(&mut self) -> Session {
        tokio::time::timeout(Duration::from_secs(20), self.sessions.recv()).await.expect("no attach within 20 s").expect("server gone")
    }
}

async fn server(key: Arc<SigningKey>, node_spki: Vec<u8>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let resume: Arc<Mutex<Vec<ResumeHint>>> = Arc::new(Mutex::new(Vec::new()));
    let (stx, srx) = mpsc::unbounded_channel();
    let r = resume.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else { return };
            let Ok(ws) = tokio_tungstenite::accept_async(tcp).await else { continue };
            let (key, spki, r, stx) = (key.clone(), node_spki.clone(), r.clone(), stx.clone());
            tokio::spawn(async move { serve(ws, key, spki, r, stx).await });
        }
    });
    Server { base, resume, sessions: srx }
}

async fn recv_hs<S>(ws: &mut S) -> HandshakeFrame
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match ws.next().await.expect("node closed the handshake").expect("handshake read") {
            Message::Text(t) => return serde_json::from_str(&t).expect("handshake frame"),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected handshake message {other:?}"),
        }
    }
}

async fn serve<S>(mut ws: S, key: Arc<SigningKey>, node_spki: Vec<u8>, resume: Arc<Mutex<Vec<ResumeHint>>>, sessions: mpsc::UnboundedSender<Session>)
where
    S: futures_util::Sink<Message> + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin + Send + 'static,
{
    let HandshakeFrame::Hello(hello) = recv_hs(&mut ws).await else { panic!("first frame isn't hello") };
    let cfp = key.fingerprint();
    let nonce_c = k2_node_proto::crypto::nonce_b64().unwrap();
    let sig = key.sign_domain(frames::DOMAIN_ATTACH_CONTROLLER, &[hello.nonce_n.as_bytes(), hello.node_fp.as_bytes(), nonce_c.as_bytes()]).unwrap();
    let ch = HandshakeFrame::Challenge(Challenge { controller_fp: cfp.clone(), nonce_c: nonce_c.clone(), sig, protocol: SERVER_PROTOCOL });
    let _ = ws.send(Message::Text(serde_json::to_string(&ch).unwrap())).await;
    let HandshakeFrame::Proof(p) = recv_hs(&mut ws).await else { panic!("expected proof") };
    assert!(
        k2_node_proto::crypto::verify_domain(&node_spki, frames::DOMAIN_ATTACH_NODE, &[nonce_c.as_bytes(), cfp.as_bytes(), hello.nonce_n.as_bytes()], &p.sig),
        "node proof doesn't verify"
    );
    let session = frames::session_id(&hello.nonce_n, &nonce_c);
    let welcome = Frame::Welcome(Welcome {
        node_id: "node-1".into(),
        name: "mini-1".into(),
        controller: "golden.k2.dev".into(),
        resume: resume.lock().unwrap().clone(),
        routes: vec![],
        narrow: None,
        server_time: NOW,
    });
    let _ = ws.send(Message::Text(frames::seal(&key, Direction::ControllerToNode, &session, 0, &welcome).unwrap())).await;
    let (in_tx, in_rx) = mpsc::unbounded_channel();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Option<Frame>>();
    let _ = sessions.send(Session { hello, rx: in_rx, tx: out_tx });
    let (mut sink, mut stream) = ws.split();
    let (mut seq, mut expected) = (1u64, 0u64);
    loop {
        tokio::select! {
            o = out_rx.recv() => match o {
                None | Some(None) => { let _ = sink.close().await; return; }
                Some(Some(f)) => {
                    let t = frames::seal(&key, Direction::ControllerToNode, &session, seq, &f).unwrap();
                    seq += 1;
                    if sink.send(Message::Text(t)).await.is_err() { return; }
                }
            },
            m = stream.next() => match m {
                Some(Ok(Message::Text(t))) => {
                    let f = frames::open(&node_spki, Direction::NodeToController, &session, expected, &t).expect("bad frame from the node");
                    expected += 1;
                    let _ = in_tx.send(f);
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                Some(Ok(_)) => {}
            }
        }
    }
}

// ── the script ───────────────────────────────────────────────────────

/// A 0.46.2-shaped plan, built from its wire form so this file compiles
/// against every version of `Plan` (later fields are optional and absent).
fn plan(job: &str, node_id: &str, ws: &str, argv: &[&str]) -> Plan {
    let limits = JobLimits { max_secs: 60, cpu_millis: None, mem_bytes: None, disk_bytes: 0, log_cap_bytes: 1 << 20 };
    serde_json::from_value(json!({
        "job_id": job,
        "node_id": node_id,
        "workspace_id": ws,
        "workspace_label": "k2",
        "requested_by": "agent:k2",
        "argv": argv,
        "env": { "GOLDEN": "1" },
        "cwd": null,
        "limits": limits,
        "src": null,
        "exclusive": false,
        "created_at": NOW,
    }))
    .expect("a 0.46.2 plan parses")
}

fn assign(p: Plan) -> Frame {
    Frame::Assign(Assign { fence: Fence { job_id: p.job_id.clone(), attempt: 1, generation: 1, plan_digest: p.digest().unwrap() }, plan: p })
}

fn about(f: &Frame, job: &str) -> bool {
    match f {
        Frame::State(s) => s.job_id == job,
        Frame::Log(l) => l.job_id == job,
        Frame::Receipt(r) => r.job_id == job,
        _ => false,
    }
}

/// The job's last state frame, its logs in sequence order, and its receipt.
fn job_record(frames: &[Frame], job: &str) -> Value {
    let mine: Vec<&Frame> = frames.iter().filter(|f| about(f, job)).collect();
    let last_state = mine.iter().rev().find(|f| matches!(f, Frame::State(_))).expect("no state frame");
    let mut logs: Vec<&frames::LogChunk> = mine.iter().filter_map(|f| if let Frame::Log(l) = f { Some(l) } else { None }).collect();
    logs.sort_by_key(|l| l.seq);
    let receipt = mine.iter().find(|f| matches!(f, Frame::Receipt(_))).expect("no receipt");
    json!({
        "state": norm(last_state, false),
        "logs": logs.iter().map(|l| serde_json::to_value(l).unwrap()).collect::<Vec<_>>(),
        "receipt": norm(receipt, false),
    })
}

fn set(v: &mut Value, ptr: &str, to: &str) {
    let shown = v.to_string();
    let slot = v.pointer_mut(ptr).unwrap_or_else(|| panic!("golden mask {ptr} missing in {shown}"));
    *slot = Value::String(to.into());
}

/// One frame as JSON with only the documented masks applied.
fn norm(f: &Frame, later_session: bool) -> Value {
    let mut v = serde_json::to_value(f).unwrap();
    match f {
        Frame::Offer(_) => {
            set(&mut v, "/protocol", "<version>");
            set(&mut v, "/node_version", "<version>");
            set(&mut v, "/ledger_id", "<random>");
            set(&mut v, "/free/mem_bytes", "<live>");
            set(&mut v, "/free/disk_free_bytes", "<live>");
            set(&mut v, "/tools", "<machine>");
            set(&mut v, "/labels/os", "<machine>");
            set(&mut v, "/labels/arch", "<machine>");
            v["labels"].as_object_mut().unwrap().remove("vm");
            if let Some(fs) = v.get_mut("features").and_then(|x| x.as_array_mut()) {
                fs.retain(|x| x != k2_node_proto::needs::FEATURE_SYSTEM_PACKAGES);
                if !recording() {
                    for f in BASE_0463_FEATURES {
                        assert_eq!(fs.iter().filter(|x| x.as_str() == Some(f)).count(), 1, "the 0.46.3 base offers {f} once: {fs:?}");
                    }
                }
                fs.retain(|x| !BASE_0463_FEATURES.iter().any(|f| x.as_str() == Some(f)));
            }
            if later_session {
                set(&mut v, "/revision", "<count>");
            }
        }
        Frame::Receipt(r) => {
            let mut body: Value = serde_json::from_str(&r.receipt.body).unwrap();
            set(&mut body, "/tools", "<machine>");
            v["receipt"] = json!({ "body": body, "sig": "<ecdsa>" });
        }
        Frame::JobsSnapshot(_) => {
            v["jobs"].as_array_mut().unwrap().sort_by(|a, b| a["job_id"].as_str().cmp(&b["job_id"].as_str()));
        }
        _ => {}
    }
    v
}

fn hello(h: &Hello) -> Value {
    let mut v = serde_json::to_value(HandshakeFrame::Hello(h.clone())).unwrap();
    set(&mut v, "/protocol", "<version>");
    set(&mut v, "/node_version", "<version>");
    set(&mut v, "/nonce_n", "<random>");
    set(&mut v, "/ledger_id", "<random>");
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn solo_frames_match_v0_46_2() {
    let ckey = Arc::new(SigningKey::from_pkcs8_pem(CONTROLLER_KEY).unwrap());
    let nkey = SigningKey::from_pkcs8_pem(NODE_KEY).unwrap();
    let mut srv = server(ckey.clone(), nkey.spki_der().to_vec()).await;

    let dir = k2_node::util::temp_dir("golden-solo");
    let layout = Layout::new(dir.join("home"), dir.join("config"));
    layout.ensure().unwrap();
    std::fs::create_dir_all(&layout.config).unwrap();
    std::fs::write(
        layout.policy(),
        "availability = \"always\"\nforeign_locks = []\nwrite_smoke_lock = \"\"\ndisk_floor_gb = 0\nworkspaces = [\"ws-1\"]\n",
    )
    .unwrap();
    std::fs::write(layout.key(), NODE_KEY).unwrap();
    let pin = Pin {
        controller_fp: ckey.fingerprint(),
        controller_public_key_pem: ckey.spki_pem(),
        routes: vec![srv.base.clone()],
        node_id: "node-1".into(),
        name: "mini-1".into(),
        enrolled_at: NOW,
        sas: "000 000".into(),
        revoked: false,
        revoked_reason: None,
        labels: BTreeMap::new(),
        confirmed: true,
    };
    k2_node::identity::write_pin(&layout.pin(), &pin).unwrap();
    let mut o = NodeOptions::new(layout.clone());
    o.dev = true;
    o.cancel_grace = Duration::from_millis(500);
    o.config_poll = Duration::from_millis(100);
    o.backoff_min = Duration::from_millis(50);
    o.backoff_max = Duration::from_millis(300);
    o.keep_awake = false;
    o.health = false;
    o.use_cgroups = false;
    o.clock = Arc::new(FixedClock);
    o.admin_socket = dir.join("no-executor.sock");
    o.facts = Some(k2_node::config::MachineFacts { cores: 8, mem_bytes: 32 << 30, disk_total_bytes: 1 << 40, has_battery: false, is_macos: false });
    let node = Node::open(o).await.unwrap();
    node.spawn_background();
    let runner = tokio::spawn(k2_node::session::run_forever(node.clone()));

    let mut got: BTreeMap<String, Value> = BTreeMap::new();

    // Session 1: hello, the first snapshot and offer.
    let mut s = srv.next().await;
    got.insert("s1.hello".into(), hello(&s.hello));
    let first = s.until(|f| matches!(f, Frame::Offer(_))).await;
    got.insert("s1.first".into(), Value::Array(first.iter().map(|f| norm(f, false)).collect()));

    // Refusals and fences: none of these reach the ledger.
    s.send(assign(plan("j-ws", "node-1", "ws-2", &["/bin/echo", "x"])));
    let r = s.until(|f| about(f, "j-ws")).await;
    got.insert("s1.workspace_refused".into(), norm(r.last().unwrap(), false));
    s.send(assign(plan("j-wn", "node-9", "ws-1", &["/bin/echo", "x"])));
    let r = s.until(|f| about(f, "j-wn")).await;
    got.insert("s1.wrong_node".into(), norm(r.last().unwrap(), false));
    let mut bad = match assign(plan("j-pm", "node-1", "ws-1", &["/bin/echo", "x"])) {
        Frame::Assign(a) => a,
        _ => unreachable!(),
    };
    bad.fence.plan_digest = format!("sha256:{}", "0".repeat(64));
    s.send(Frame::Assign(bad));
    let r = s.until(|f| about(f, "j-pm")).await;
    got.insert("s1.plan_mismatch".into(), norm(r.last().unwrap(), false));
    s.send(Frame::Cancel(Cancel { job_id: "j-none".into(), generation: 1, reason: "cancelled".into() }));
    let r = s.until(|f| about(f, "j-none")).await;
    got.insert("s1.cancel_unknown".into(), norm(r.last().unwrap(), false));

    // A job that runs to the end (stdout, then stderr).
    s.send(assign(plan("j1", "node-1", "ws-1", &["/bin/sh", "-c", "printf 'out\\n'; sleep 0.3; printf 'err\\n' >&2"])));
    let r = s.until(|f| matches!(f, Frame::Receipt(x) if x.job_id == "j1")).await;
    got.insert("s1.j1".into(), job_record(&r, "j1"));

    // A job cancelled while it runs.
    s.send(assign(plan("j2", "node-1", "ws-1", &["/bin/sh", "-c", "sleep 30"])));
    s.until(|f| matches!(f, Frame::State(x) if x.job_id == "j2" && x.state == JobState::Running)).await;
    s.send(Frame::Cancel(Cancel { job_id: "j2".into(), generation: 1, reason: "cancelled".into() }));
    let r = s.until(|f| matches!(f, Frame::Receipt(x) if x.job_id == "j2")).await;
    got.insert("s1.j2_cancelled".into(), job_record(&r, "j2"));

    // Session 2: the server drops and comes back asking to resume j1
    // after its first log record.
    *srv.resume.lock().unwrap() = vec![ResumeHint { job_id: "j1".into(), generation: 1, since_seq: 1 }];
    s.close();
    let mut s2 = srv.next().await;
    got.insert("s2.hello".into(), hello(&s2.hello));
    let first = s2.until(|f| matches!(f, Frame::Offer(_))).await;
    got.insert("s2.first".into(), Value::Array(first.iter().map(|f| norm(f, true)).collect()));
    let r = s2.until(|f| matches!(f, Frame::Receipt(x) if x.job_id == "j1")).await;
    got.insert("s2.resume_j1".into(), job_record(&r, "j1"));

    runner.abort();
    node.stop_all(JobState::Cancelled, "test_over");

    let text = serde_json::to_string_pretty(&got).unwrap() + "\n";
    if recording() {
        let out = std::env::var("GOLDEN_RECORD").unwrap();
        std::fs::write(&out, &text).unwrap_or_else(|e| panic!("write {out}: {e}"));
        eprintln!("recorded {out}");
        return;
    }
    let want_text = std::fs::read_to_string(GOLDEN).unwrap_or_else(|e| panic!("read {GOLDEN}: {e} (record it from the v0.46.2 sources)"));
    let want: BTreeMap<String, Value> = serde_json::from_str(&want_text).unwrap();
    assert_eq!(want.keys().collect::<Vec<_>>(), got.keys().collect::<Vec<_>>(), "golden steps differ");
    let mut diffs = Vec::new();
    for (k, w) in &want {
        let g = &got[k];
        if w != g {
            diffs.push(format!(
                "solo frame {k} differs from 0.46.2\nwant: {}\n got: {}",
                serde_json::to_string_pretty(w).unwrap(),
                serde_json::to_string_pretty(g).unwrap()
            ));
        }
    }
    assert!(diffs.is_empty(), "{} step(s) differ:\n{}", diffs.len(), diffs.join("\n\n"));
}
