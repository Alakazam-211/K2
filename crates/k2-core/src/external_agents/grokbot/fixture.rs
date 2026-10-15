//! Test fixtures for the Grok Bot adapter: a fake `~/sand-data` tree (in a
//! temp HOME) and a fake gateway HTTP server. Shapes follow the sanitized
//! samples from the grokbot box (2026-10-15). Used by k2-core unit tests
//! and k2-daemon integration tests (`test-util`).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use serde_json::{json, Value};

use super::schema;

// ── open recorder ───────────────────────────────────────────────────────

static OPENED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Called by the adapter's single file door for every path it opens.
pub(crate) fn record_open(p: &Path) {
    OPENED.lock().unwrap_or_else(|e| e.into_inner()).push(p.to_path_buf());
}

/// Every path the adapter opened under `root` so far (other tests' roots
/// are filtered out; tests run in parallel).
pub fn opened_under(root: &Path) -> Vec<PathBuf> {
    OPENED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|p| p.starts_with(root))
        .cloned()
        .collect()
}

// ── fake sand-data ──────────────────────────────────────────────────────

/// The secret files Grok Bot keeps next to the data K2 reads. The fixture
/// writes them so tests can prove the adapter never opens them.
pub const SECRET_FILES: &[&str] = &[
    "box-secrets.json",
    "host-secrets.json",
    "chrome-cookie-seed.json",
    "teach-queue-key.json",
    "connector-secrets/github.json",
    "settings.json",
    "send-acceptance.json",
];

pub const FAKE_TOKEN: &str = "sandtok-FAKE-0123456789abcdef";

/// A fake `~/sand-data` under `home`.
pub struct FakeSandData {
    pub root: PathBuf,
}

impl FakeSandData {
    /// Create the tree with `gateway.json` pointing at `port`, plus the
    /// secret decoys.
    pub fn create(home: &Path, port: u16) -> Self {
        let root = home.join(schema::DATA_DIR);
        std::fs::create_dir_all(root.join(schema::AGENTS_DIR)).expect("mk sand-data/agents");
        let s = Self { root };
        s.write_gateway(port);
        for f in SECRET_FILES {
            let p = s.root.join(f);
            std::fs::create_dir_all(p.parent().expect("parent")).expect("mk secret dir");
            std::fs::write(&p, b"{\"secret\":\"DO-NOT-READ\"}").expect("write secret decoy");
        }
        // Per-agent noise K2 must not open either.
        std::fs::write(s.root.join("search-index.db"), b"not for k2").expect("write");
        s
    }

    pub fn write_gateway(&self, port: u16) {
        std::fs::write(
            self.root.join(schema::GATEWAY_FILE),
            serde_json::to_vec(&json!({
                "port": port, "pid": 12345, "startedAt": 1_791_993_525_941_i64,
                "scheme": "http", "host": "127.0.0.1", "token": FAKE_TOKEN,
            }))
            .expect("json"),
        )
        .expect("write gateway.json");
    }

    pub fn agent_dir(&self, id: &str) -> PathBuf {
        self.root.join(schema::AGENTS_DIR).join(id)
    }

    /// Add an agent folder with `profile.json` and an empty `store.db`
    /// (real schema, WAL), plus decoy files K2 must not open.
    pub fn add_agent(&self, id: &str, name: &str, title: &str) {
        let d = self.agent_dir(id);
        std::fs::create_dir_all(d.join("attachments")).expect("mk agent dir");
        std::fs::write(
            d.join(schema::PROFILE_FILE),
            serde_json::to_vec(&json!({
                "name": name, "description": "desc", "title": title,
                "avatarShape": "circle", "avatarColor": "#7c3aed",
                "serverId": "srv", "harness": "grok",
            }))
            .expect("json"),
        )
        .expect("write profile");
        std::fs::write(d.join("settings.json"), b"{\"private\":true}").expect("write");
        std::fs::write(d.join("conversation-blobs.db"), b"private").expect("write");
        std::fs::write(d.join("audit.jsonl"), b"{}\n").expect("write");
        let c = self.writer(id);
        c.execute_batch(
            "CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
             CREATE TABLE IF NOT EXISTS blobs (id TEXT PRIMARY KEY, data BLOB NOT NULL) STRICT;
             CREATE TABLE IF NOT EXISTS transcript_entries (
               seq INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE, entry TEXT NOT NULL) STRICT;
             CREATE INDEX IF NOT EXISTS idx_transcript_window ON transcript_entries(seq, entry)
               WHERE json_extract(entry, '$.kind') != 'tool-call'
                 AND COALESCE(json_extract(entry, '$.branched'), 0) != 1;",
        )
        .expect("create store schema");
    }

    /// An empty-shell agent folder (no profile, no store).
    pub fn add_shell(&self, id: &str) {
        std::fs::create_dir_all(self.agent_dir(id)).expect("mk shell");
    }

    /// A read-write connection to an agent's store (WAL), like Grok Bot's
    /// own writer.
    pub fn writer(&self, id: &str) -> Connection {
        let c = Connection::open(self.agent_dir(id).join(schema::STORE_FILE)).expect("open store rw");
        c.pragma_update(None, "journal_mode", "WAL").expect("wal");
        c
    }

    /// Append one entry; returns its seq.
    pub fn add_entry(&self, agent: &str, entry: Value) -> i64 {
        let c = self.writer(agent);
        let id = entry
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        c.execute(
            "INSERT INTO transcript_entries (id, entry) VALUES (?1, ?2)",
            rusqlite::params![id, entry.to_string()],
        )
        .expect("insert entry");
        c.last_insert_rowid()
    }

    /// The sample rows from the grokbot box (fake text, real shapes).
    pub fn add_sample_chat(&self, agent: &str) {
        for e in sample_entries() {
            self.add_entry(agent, e);
        }
    }
}

/// Sample entries (one of each observed kind). Visible chat: m1 (human),
/// m2 (peer, from Cortana), s1 (agent text), m3 (agent → Jarvis), s2
/// (widget → open in app). Hidden: e1, x1, f1, a tool-call and a branched
/// row.
pub fn sample_entries() -> Vec<Value> {
    vec![
        json!({"kind":"message","id":"m1","role":"user","content":"Can you check the build?","isStreaming":false,"clientNonce":"n1","timestampMs":1_791_990_000_000_i64}),
        json!({"kind":"message","id":"m2","role":"user","content":"[from Cortana] [k2g] hi","isStreaming":false,"timestampMs":1_791_990_001_000_i64,"fromAgent":{"id":"0e5f5de8-0000-0000-0000-000000000000","name":"Cortana"}}),
        json!({"kind":"tool-call","id":"t1","name":"Shell","timestampMs":1_791_990_002_000_i64}),
        json!({"kind":"send-message","id":"s1","message":{"type":"text","content":"Build is green."},"timestampMs":1_791_990_060_000_i64}),
        json!({"kind":"message","id":"m3","role":"assistant","content":"Pinging Jarvis…","isStreaming":false,"timestampMs":1_791_990_070_000_i64,"toAgent":{"id":"1a2b3c4d-0000-0000-0000-000000000000","name":"Jarvis","kind":"agent"}}),
        json!({"kind":"message","id":"b1","role":"assistant","content":"branched away","branched":1,"timestampMs":1_791_990_071_000_i64}),
        json!({"kind":"send-message","id":"s2","message":{"type":"widget","content":"<opaque>"},"timestampMs":1_791_990_080_000_i64}),
        json!({"kind":"event","id":"e1","event":{"type":"automation","action":"run","automationId":"a","automationName":"n"},"timestampMs":1_791_990_090_000_i64}),
        json!({"kind":"spend-initiation","version":1,"id":"x1","agentId":"u","requestId":"r","timestampMs":0,"initiation":{"type":"t","id":"i","timestampMs":0,"initiatingMessageCount":1}}),
        json!({"kind":"feedback","id":"f1","requestId":"r","state":"s","timestampMs":0}),
    ]
}

/// One `listAgents` item in the sampled shape.
pub fn list_item(id: &str, name: &str) -> Value {
    json!({
        "id": id, "name": name, "title": "Assistant", "description": "d",
        "avatarDataUrl": "data:image/png;base64,iVBORw0KGgo=", "avatarVersion": null,
        "avatarShape": "circle", "avatarColor": "#7c3aed",
        "createdAt": 0, "updatedAt": 0, "localActivityAt": 0, "lastActivityAt": 0, "lastViewedAt": 0,
        "path": "/home/box/sand-data/agents/x",
        "isActive": true, "isRunning": true, "isRunningTurn": false, "isComposingMessage": false,
        "awaitingUserResponse": null, "hasUnread": false, "unreadCount": 0,
        "lastEntry": {"kind": "message", "text": "t"},
        "lastMessageId": "m", "lastMessagePreview": "p", "lastMessagePreviewSource": null,
        "lastMessageAuthorId": null, "newestEntryId": "e0", "lastMessageTaskId": null,
        "pushMessageContent": null, "notificationsEnabled": true, "notifyOnUpdatesEnabled": true,
        "isHiddenFromSidebar": false, "voiceId": null, "voiceSpeed": 1, "voiceLanguage": "en",
        "origin": "local", "isGroup": false, "memberIds": [], "harness": "grok",
        "snapshotEpoch": "ep1", "snapshotSeq": 0
    })
}

// ── fake gateway ────────────────────────────────────────────────────────

/// One request the fake gateway received.
#[derive(Debug, Clone)]
pub struct SeenRequest {
    pub method: String,
    pub path: String,
    pub authorization: Option<String>,
    pub body: Value,
}

#[derive(Debug)]
pub struct GatewayState {
    pub agents: Value,
    pub busy: bool,
    pub active_agent_id: Option<String>,
    pub send_status: u16,
    pub seen: Vec<SeenRequest>,
}

/// A loopback HTTP server answering `/health`, `/api/listAgents` and
/// `/api/sendPrompt` like Grok Bot's gateway (bearer checked).
pub struct FakeGateway {
    pub port: u16,
    pub state: Arc<Mutex<GatewayState>>,
    stop: Arc<AtomicBool>,
}

impl FakeGateway {
    pub fn start(agents: Value) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake gateway");
        let port = listener.local_addr().expect("addr").port();
        let state = Arc::new(Mutex::new(GatewayState {
            agents,
            busy: false,
            active_agent_id: None,
            send_status: 200,
            seen: Vec::new(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (st, sp) = (state.clone(), stop.clone());
        std::thread::Builder::new()
            .name("fake-sand-gateway".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    if sp.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Ok(c) = conn {
                        serve_one(c, &st);
                    }
                }
            })
            .expect("spawn fake gateway");
        Self { port, state, stop }
    }

    pub fn seen(&self) -> Vec<SeenRequest> {
        self.state.lock().expect("state").seen.clone()
    }

    pub fn set(&self, f: impl FnOnce(&mut GatewayState)) {
        f(&mut self.state.lock().expect("state"));
    }
}

impl Drop for FakeGateway {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn serve_one(stream: TcpStream, state: &Arc<Mutex<GatewayState>>) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.is_empty() {
        return;
    }
    let mut it = line.split_whitespace();
    let method = it.next().unwrap_or("").to_string();
    let path = it.next().unwrap_or("").to_string();
    let mut len = 0usize;
    let mut auth = None;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).is_err() || h == "\r\n" || h.is_empty() {
            break;
        }
        let lower = h.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
        if lower.starts_with("authorization:") {
            auth = Some(h["authorization:".len()..].trim().to_string());
        }
    }
    let mut body = vec![0u8; len];
    if len > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let authed = auth.as_deref() == Some(&format!("Bearer {FAKE_TOKEN}"));
    let (status, out) = {
        let mut s = state.lock().expect("state");
        s.seen.push(SeenRequest {
            method: method.clone(),
            path: path.clone(),
            authorization: auth.clone(),
            body: body.clone(),
        });
        match (method.as_str(), path.as_str()) {
            ("GET", "/health") => (
                200,
                json!({"ok":true,"pid":1,"isBusy":s.busy,"activeAgentId":s.active_agent_id,
                       "startedAt":1,"lastBusyAtMs":1_791_993_525_941_i64,"agentCpuSaturatedSinceMs":null}),
            ),
            ("POST", "/api/listAgents") if authed => (200, s.agents.clone()),
            ("POST", "/api/sendPrompt") if authed => (s.send_status, json!({"accepted": true})),
            ("POST", _) => (401, json!({"error":"unauthorized"})),
            _ => (404, json!({"error":"not found"})),
        }
    };
    let body = out.to_string();
    let mut stream = stream;
    let _ = write!(
        stream,
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.flush();
}
