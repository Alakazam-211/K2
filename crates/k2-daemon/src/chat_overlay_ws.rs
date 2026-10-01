//! `GET /cli/chat/transcript` — harness-agnostic session-log tail.
//!
//! The window opens this socket only while Chat is the visible view.
//! Turns are not published on the grid socket, `/cli/sessions/events`,
//! or `/cli/overlay/events`. The tail runs only while a subscriber holds
//! the socket. It does not register the harness file on the project-root
//! watcher.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use k2_core::chat_continue::locate_continue_transcript;
use k2_core::chat_overlay::{ChatTurn, TranscriptCursor};
use k2_core::log_debug;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::tungstenite::Message;

/// Wire path. Not grid, session-events, overlay events, or fs events.
pub const CHAT_TRANSCRIPT_WS_PATH: &str = "/cli/chat/transcript";

const TAIL_POLL: Duration = Duration::from_millis(200);

struct SenderSlot {
    id: u64,
    tx: mpsc::UnboundedSender<String>,
}

struct TailSlot {
    senders: Vec<SenderSlot>,
    turns: Vec<ChatTurn>,
    cancel: std::sync::Arc<AtomicBool>,
    notify: std::sync::Arc<Notify>,
}

struct TailHub {
    slots: HashMap<String, TailSlot>,
}

fn hub() -> &'static Mutex<TailHub> {
    static HUB: OnceLock<Mutex<TailHub>> = OnceLock::new();
    HUB.get_or_init(|| {
        Mutex::new(TailHub {
            slots: HashMap::new(),
        })
    })
}

fn lock_hub() -> std::sync::MutexGuard<'static, TailHub> {
    hub().lock().unwrap_or_else(|err| err.into_inner())
}

static NEXT_SUB: AtomicU64 = AtomicU64::new(1);
static TAIL_TASKS: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
pub(crate) fn active_chat_tail_count() -> usize {
    lock_hub().slots.len()
}

#[cfg(test)]
pub(crate) fn chat_tail_tasks() -> usize {
    TAIL_TASKS.load(Ordering::SeqCst)
}

fn tail_key(provider: &str, conversation: &str, project: &str) -> String {
    format!("{provider}\n{conversation}\n{project}")
}

pub struct TailSubscription {
    key: String,
    id: u64,
    pub rx: mpsc::UnboundedReceiver<String>,
}

impl Drop for TailSubscription {
    fn drop(&mut self) {
        unsubscribe(&self.key, self.id);
    }
}

fn is_v1_provider(provider: &str) -> bool {
    matches!(provider, "claude" | "codex" | "grok" | "gemini")
}

/// Start or join a tail. Empty conversation does not watch a file.
pub fn subscribe_chat_tail(
    provider: &str,
    conversation: &str,
    project: &str,
) -> Result<TailSubscription, &'static str> {
    let provider = provider.trim().to_ascii_lowercase();
    let conversation = conversation.trim().to_string();
    if conversation.is_empty() {
        return Err("conversation required");
    }
    if !is_v1_provider(&provider) {
        return Err("unsupported provider");
    }
    let project = project.to_string();
    let key = tail_key(&provider, &conversation, &project);
    let id = NEXT_SUB.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::unbounded_channel();
    let snapshot = {
        let mut hub = lock_hub();
        if let Some(slot) = hub.slots.get_mut(&key) {
            let snapshot = slot.turns.clone();
            slot.senders.push(SenderSlot { id, tx: tx.clone() });
            snapshot
        } else {
            let cancel = std::sync::Arc::new(AtomicBool::new(false));
            let notify = std::sync::Arc::new(Notify::new());
            hub.slots.insert(
                key.clone(),
                TailSlot {
                    senders: vec![SenderSlot { id, tx: tx.clone() }],
                    turns: Vec::new(),
                    cancel: cancel.clone(),
                    notify: notify.clone(),
                },
            );
            let key_task = key.clone();
            tokio::spawn(async move {
                run_tail(key_task, provider, conversation, project, cancel, notify).await;
            });
            Vec::new()
        }
    };
    if !snapshot.is_empty() {
        let _ = tx.send(snapshot_frame(&snapshot));
    }
    Ok(TailSubscription { key, id, rx })
}

fn unsubscribe(key: &str, id: u64) {
    let mut hub = lock_hub();
    let Some(slot) = hub.slots.get_mut(key) else {
        return;
    };
    slot.senders.retain(|sender| sender.id != id);
    if slot.senders.is_empty() {
        slot.cancel.store(true, Ordering::SeqCst);
        slot.notify.notify_waiters();
        hub.slots.remove(key);
    }
}

fn snapshot_frame(turns: &[ChatTurn]) -> String {
    serde_json::json!({
        "kind": "snapshot",
        "turns": turns,
    })
    .to_string()
}

fn turn_frame(turn: &ChatTurn) -> String {
    serde_json::json!({
        "kind": "turn",
        "turn": turn,
    })
    .to_string()
}

struct FileFollow {
    path: Option<PathBuf>,
    offset: u64,
    pending: String,
    cursor: TranscriptCursor,
}

struct FollowUpdate {
    reset: bool,
    changed: Vec<ChatTurn>,
    all: Vec<ChatTurn>,
}

impl FileFollow {
    fn new() -> Self {
        Self {
            path: None,
            offset: 0,
            pending: String::new(),
            cursor: TranscriptCursor::default(),
        }
    }

    fn poll(&mut self, provider: &str, conversation: &str, project: &str) -> FollowUpdate {
        let next = locate_continue_transcript(provider, conversation, project);
        let mut reset = false;
        if next != self.path {
            self.path = next;
            self.offset = 0;
            self.pending.clear();
            self.cursor.clear();
            reset = true;
        }
        let Some(path) = self.path.clone() else {
            return FollowUpdate {
                reset,
                changed: Vec::new(),
                all: self.cursor.turns().to_vec(),
            };
        };
        let mut changed = Vec::new();
        if let Some(chunk) = read_new(&path, &mut self.offset) {
            if chunk.is_empty() && reset {
                return FollowUpdate {
                    reset,
                    changed,
                    all: self.cursor.turns().to_vec(),
                };
            }
            self.pending.push_str(&chunk);
            while let Some(idx) = self.pending.find('\n') {
                let line: String = self.pending.drain(..=idx).collect();
                changed.extend(self.cursor.push_line(provider, line.trim_end_matches('\n')));
            }
        }
        FollowUpdate {
            reset,
            changed,
            all: self.cursor.turns().to_vec(),
        }
    }
}

fn read_new(path: &Path, offset: &mut u64) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len < *offset {
        *offset = 0;
    }
    if len == *offset {
        return Some(String::new());
    }
    file.seek(SeekFrom::Start(*offset)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    *offset += buf.len() as u64;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn publish(key: &str, update: FollowUpdate) {
    let senders = {
        let mut hub = lock_hub();
        let Some(slot) = hub.slots.get_mut(key) else {
            return;
        };
        slot.turns = update.all;
        slot.senders
            .iter()
            .map(|sender| sender.tx.clone())
            .collect::<Vec<_>>()
    };
    if update.reset {
        for tx in &senders {
            let _ = tx.send(r#"{"kind":"reset"}"#.to_string());
        }
    }
    for turn in &update.changed {
        let frame = turn_frame(turn);
        for tx in &senders {
            let _ = tx.send(frame.clone());
        }
    }
}

async fn run_tail(
    key: String,
    provider: String,
    conversation: String,
    project: String,
    cancel: std::sync::Arc<AtomicBool>,
    notify: std::sync::Arc<Notify>,
) {
    TAIL_TASKS.fetch_add(1, Ordering::SeqCst);
    let mut follow = FileFollow::new();
    loop {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let update = follow.poll(&provider, &conversation, &project);
        if update.reset || !update.changed.is_empty() {
            publish(&key, update);
        }
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        tokio::select! {
            _ = notify.notified() => {}
            _ = tokio::time::sleep(TAIL_POLL) => {}
        }
    }
    TAIL_TASKS.fetch_sub(1, Ordering::SeqCst);
}

fn project_path_for_agent(agent: &str) -> String {
    crate::v2_session_map::lookup_by_agent_name(agent)
        .and_then(|session| {
            session
                .cwd
                .as_ref()
                .map(|cwd| cwd.to_string_lossy().into_owned())
        })
        .unwrap_or_default()
}

async fn write_http(stream: &mut TcpStream, status: &str, body: &str) {
    // The dispatcher only peeked the request. Consume it before answering
    // or a close-with-unread-data RST drops the status line.
    let mut buf = [0u8; 8192];
    let _ = tokio::io::AsyncReadExt::read(stream, &mut buf).await;
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = tokio::io::AsyncWriteExt::write_all(stream, resp.as_bytes()).await;
    let _ = tokio::io::AsyncWriteExt::flush(stream).await;
}

/// Dispatcher already checked `token_ok` and refused skin tokens.
/// Missing conversation id returns 400 and does not watch a file.
pub async fn serve_chat_transcript_connection(
    stream: &mut TcpStream,
    params: HashMap<String, String>,
) {
    let provider = params
        .get("provider")
        .map(|s| s.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let conversation = params
        .get("conversation")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if conversation.is_empty() {
        log_debug!("[daemon/chat_overlay_ws] missing conversation=");
        write_http(
            stream,
            "400 Bad Request",
            r#"{"error":"conversation required"}"#,
        )
        .await;
        return;
    }
    if !is_v1_provider(&provider) {
        write_http(
            stream,
            "400 Bad Request",
            r#"{"error":"unsupported provider"}"#,
        )
        .await;
        return;
    }
    let project = params
        .get("agent")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(project_path_for_agent)
        .unwrap_or_default();

    let ws = match tokio_tungstenite::accept_async(&mut *stream).await {
        Ok(ws) => ws,
        Err(err) => {
            log_debug!("[daemon/chat_overlay_ws] handshake failed: {err}");
            return;
        }
    };
    let mut sub = match subscribe_chat_tail(&provider, &conversation, &project) {
        Ok(sub) => sub,
        Err(err) => {
            log_debug!("[daemon/chat_overlay_ws] subscribe refused: {err}");
            return;
        }
    };
    let (mut write, mut read) = ws.split();
    loop {
        tokio::select! {
            incoming = read.next() => {
                match incoming {
                    Some(Ok(Message::Ping(payload))) => {
                        if write.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(Message::Pong(_))) | Some(Ok(Message::Text(_)))
                    | Some(Ok(Message::Binary(_))) | Some(Ok(Message::Frame(_))) => {}
                }
            }
            event = sub.rx.recv() => {
                match event {
                    Some(frame) => {
                        if write.send(Message::Text(frame)).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::chat_history::claude_project_hash;
    use k2_core::chat_overlay::ChatBlock;

    fn prod_source() -> &'static str {
        let src = include_str!("chat_overlay_ws.rs");
        src.split("#[cfg(test)]").next().expect("prod source")
    }

    #[test]
    fn path_is_not_on_the_other_buses_or_post_allowed() {
        assert_eq!(CHAT_TRANSCRIPT_WS_PATH, "/cli/chat/transcript");
        assert_ne!(CHAT_TRANSCRIPT_WS_PATH, "/cli/sessions/grid");
        assert_ne!(CHAT_TRANSCRIPT_WS_PATH, "/cli/sessions/events");
        assert_ne!(CHAT_TRANSCRIPT_WS_PATH, "/cli/overlay/events");
        assert_ne!(CHAT_TRANSCRIPT_WS_PATH, "/cli/fs/events");
        let dispatcher = include_str!("routes/dispatcher.rs");
        // The POST allowlist is the route policy table.
        assert!(
            !crate::routes::route_policy::post_allowed(CHAT_TRANSCRIPT_WS_PATH),
            "transcript socket must not be POST-allowed"
        );
        assert!(dispatcher.contains("crate::chat_overlay_ws::CHAT_TRANSCRIPT_WS_PATH"));
        assert!(crate::skin_gateway::never_proxy(CHAT_TRANSCRIPT_WS_PATH));
        assert!(crate::skin_gateway::never_proxy(&format!(
            "{CHAT_TRANSCRIPT_WS_PATH}?provider=claude"
        )));
        assert!(!crate::skin_gateway::allowlisted_ws(
            CHAT_TRANSCRIPT_WS_PATH
        ));
        assert!(!crate::skin_gateway::allowlisted_ws(&format!(
            "{CHAT_TRANSCRIPT_WS_PATH}?conversation=abc"
        )));
        let prod = prod_source();
        assert!(!prod.contains("fs_live"));
        assert!(!prod.contains("session_events"));
        assert!(!prod.contains("build_continue_seed"));
    }

    #[test]
    fn empty_conversation_does_not_watch() {
        match subscribe_chat_tail("claude", "  ", "/tmp/work") {
            Ok(_) => panic!("empty conversation subscribed"),
            Err(err) => assert_eq!(err, "conversation required"),
        }
    }

    #[tokio::test]
    async fn tail_emits_appended_bytes_and_stops_when_the_last_subscriber_leaves() {
        let home = crate::test_support::TempHome::new();
        let project = home.path().join("work");
        std::fs::create_dir_all(&project).expect("project");
        let project_s = project.to_string_lossy().into_owned();
        let hash = claude_project_hash(&project_s);
        let dir = home.path().join(".claude").join("projects").join(hash);
        std::fs::create_dir_all(&dir).expect("claude dir");
        let sid = "sess-tail-1";
        let path = dir.join(format!("{sid}.jsonl"));
        std::fs::write(
            &path,
            "{\"type\":\"user\",\"uuid\":\"u1\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":{\"content\":\"Hello\"}}\n",
        )
        .expect("seed");

        let mut sub = subscribe_chat_tail("claude", sid, &project_s).expect("subscribe");
        assert_eq!(active_chat_tail_count(), 1);
        let first = next_turn(&mut sub.rx).await;
        assert_eq!(first.id, "u1");
        assert!(first.blocks.iter().any(|block| matches!(
            block,
            ChatBlock::Text { text } if text == "Hello"
        )));

        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("append");
        file.write_all(
            b"{\"type\":\"assistant\",\"uuid\":\"a1\",\"message\":{\"id\":\"msg-a\",\"content\":[{\"type\":\"text\",\"text\":\"Hi\"}]}}\n",
        )
        .expect("write");
        drop(file);
        let second = next_turn(&mut sub.rx).await;
        assert_eq!(second.id, "msg-a");
        assert_eq!(second.role, "assistant");

        drop(sub);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if active_chat_tail_count() == 0 && chat_tail_tasks() == 0 {
                break;
            }
            if tokio::time::Instant::now() > deadline {
                panic!(
                    "tail still running: slots={} tasks={}",
                    active_chat_tail_count(),
                    chat_tail_tasks()
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("append after stop")
            .write_all(
                b"{\"type\":\"user\",\"uuid\":\"u9\",\"message\":{\"content\":\"AFTER_STOP\"}}\n",
            )
            .expect("write after stop");
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(active_chat_tail_count(), 0);
        assert_eq!(chat_tail_tasks(), 0);
    }

    async fn next_turn(rx: &mut mpsc::UnboundedReceiver<String>) -> ChatTurn {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                panic!("timed out waiting for a chat turn");
            }
            let msg = tokio::time::timeout(deadline - now, rx.recv())
                .await
                .expect("turn wait")
                .expect("tail channel closed");
            let parsed: serde_json::Value = serde_json::from_str(&msg).expect("frame json");
            if parsed.get("kind").and_then(|k| k.as_str()) != Some("turn") {
                continue;
            }
            let turn = parsed.get("turn").cloned().expect("turn field");
            return serde_json::from_value(turn).expect("turn decode");
        }
    }
}
