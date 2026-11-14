//! K2's own Sew gateway client (prd-home-people-channels-v1 §14.2, HP-37):
//! one person's websocket on Sew's loopback listener, with K2's
//! `tokio-tungstenite` (no `sew-client` gateway feature, no new crates).
//! The kit feed (`voice::kit_feed`) and Home chat's follower
//! (`home_chat::follower`) both open their sockets here.
//!
//! The handshake: TCP to the loopback listener, the websocket upgrade on
//! `/api/v1/gateway?v=1&encoding=json`, Sew's HELLO (op 8) with the
//! heartbeat interval, then the caller's IDENTIFY (op 2, a gateway ticket
//! taken as the person) or RESUME (op 4), then READY or RESUMED. Frames are
//! `{"op", "d", "s", "t"}`; nothing here logs a frame body (a voice server
//! update carries a media token).

use std::net::SocketAddr;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

/// Gateway opcodes K2 uses (Sew C7; stable wire numbers).
pub mod op {
    pub const DISPATCH: u64 = 0;
    pub const HEARTBEAT: u64 = 1;
    pub const IDENTIFY: u64 = 2;
    pub const RESUME: u64 = 4;
    pub const SUBSCRIBE: u64 = 5;
    pub const RECONNECT: u64 = 6;
    pub const INVALID_SESSION: u64 = 7;
    pub const HELLO: u64 = 8;
    pub const TYPING: u64 = 10;
    pub const REAUTH: u64 = 11;
}

/// The ticket route: a single-use gateway ticket for the caller's session.
pub const TICKET: &str = "/api/v1/gateway/ticket";

/// A gateway socket after HELLO.
pub struct Socket {
    pub ws: WebSocketStream<TcpStream>,
    /// HELLO's `heartbeat_interval_ms` (clamped 1–120 s).
    pub beat: Duration,
}

/// Why opening failed (each maps to the caller's own refusal).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    Unreachable,
    Refused,
    Closed,
    NoHello,
}

/// The next text frame as JSON (`None`: the socket closed or failed).
pub async fn next_frame(ws: &mut WebSocketStream<TcpStream>) -> Option<Value> {
    loop {
        match ws.next().await? {
            Ok(Message::Text(t)) => return serde_json::from_str(&t).ok(),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => continue,
        }
    }
}

/// Connect to Sew's gateway at `sa` and read HELLO.
pub async fn connect(sa: SocketAddr) -> Result<Socket, OpenError> {
    let tcp = TcpStream::connect(sa).await.map_err(|_| OpenError::Unreachable)?;
    let (mut ws, _) = tokio_tungstenite::client_async(format!("ws://{sa}/api/v1/gateway?v=1&encoding=json"), tcp)
        .await
        .map_err(|_| OpenError::Refused)?;
    let hello = next_frame(&mut ws).await.ok_or(OpenError::Closed)?;
    if hello.get("op").and_then(Value::as_u64) != Some(op::HELLO) {
        return Err(OpenError::NoHello);
    }
    let beat_ms = hello.pointer("/d/heartbeat_interval_ms").and_then(Value::as_u64).unwrap_or(30_000).clamp(1_000, 120_000);
    Ok(Socket { ws, beat: Duration::from_millis(beat_ms) })
}

/// Op 2 IDENTIFY with a gateway ticket.
pub fn identify(ticket: &str, client: &str) -> Value {
    json!({
        "op": op::IDENTIFY,
        "d": { "ticket": ticket, "client": { "name": client, "version": env!("CARGO_PKG_VERSION") }, "capabilities": 0 },
    })
}

/// Op 4 RESUME with a gateway ticket, the gateway session and the last seq.
pub fn resume(ticket: &str, session_id: &str, seq: u64) -> Value {
    json!({ "op": op::RESUME, "d": { "ticket": ticket, "session_id": session_id, "seq": seq } })
}

/// Op 11 REAUTH: the renewed session token on the live socket (HP-28).
pub fn reauth(token: &str) -> Value {
    json!({ "op": op::REAUTH, "d": { "token": token } })
}

pub async fn send(ws: &mut WebSocketStream<TcpStream>, frame: &Value) -> Result<(), OpenError> {
    ws.send(Message::Text(frame.to_string())).await.map_err(|_| OpenError::Closed)
}

/// What the first dispatch after IDENTIFY or RESUME was.
#[derive(Debug, Clone, PartialEq)]
pub enum Started {
    /// READY: its body and seq.
    Ready { d: Value, s: Option<u64> },
    /// RESUMED: Sew replays what was missed next.
    Resumed,
    /// INVALID_SESSION, and whether Sew says it may resume.
    Invalid { resumable: bool },
}

/// Read until READY, RESUMED or INVALID_SESSION (`None`: the socket closed).
pub async fn started(ws: &mut WebSocketStream<TcpStream>) -> Option<Started> {
    loop {
        let f = next_frame(ws).await?;
        match f.get("op").and_then(Value::as_u64) {
            Some(op::DISPATCH) => match f.get("t").and_then(Value::as_str) {
                Some("ready") => {
                    return Some(Started::Ready { d: f.get("d").cloned().unwrap_or(Value::Null), s: f.get("s").and_then(Value::as_u64) })
                }
                Some("resumed") => return Some(Started::Resumed),
                _ => continue,
            },
            Some(op::INVALID_SESSION) => {
                return Some(Started::Invalid { resumable: f.pointer("/d/resumable").and_then(Value::as_bool).unwrap_or(false) })
            }
            _ => continue,
        }
    }
}
