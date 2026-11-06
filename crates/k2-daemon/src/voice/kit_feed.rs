//! The kit's event feed: `GET /_k2/voice/sew/feed` (a WebSocket) on the
//! call door, for the person behind the request (`kit_door`).
//!
//! **One upstream per feed.** Sew filters events per person and often per
//! session (self-only bodies, a media token, per-session policy and perms),
//! so K2 never shares a stream: each feed is the person's own Sew session
//! socket (a gateway ticket from their own `sews_` session, IDENTIFY with
//! `capabilities: 0`, Sew's heartbeats), and K2 forwards only
//! [`EVENTS`].
//!
//! **What the phone gets.** Sew's dispatch frames, `{"op": 0, "t", "s",
//! "d"}`, with `s` renumbered from 1 so it stays contiguous after K2 drops
//! names. The first frame is always Sew's READY (`t: "ready"`), whole but
//! for `resume_url` (blanked: K2 never resumes, and it names Sew's loopback
//! listener). The 101 is written only after READY, so a refusal is still an
//! HTTP answer. The phone sends nothing; K2 pings it. Closes: [`CLOSE_RESET`]
//! (Sew's socket ended or a check failed: reconnect, a fresh READY follows)
//! and [`CLOSE_LOGIN_ENDED`] (the K2 login ended: sign in again).
//!
//! **K2's own narrowing** (Sew's §2.3 table, on top of Sew's filter): a
//! voice state, recording, call or channel event passes only when it names a
//! channel the person can see ([`gate`]); `voice_state_updated` for the
//! person themselves always passes. A channel is visible when READY listed
//! it, or when `GET /api/v1/channels/{id}` as the person answers 200 (403
//! and 404 are hidden). Perms and membership changes drop what K2 learned.
//! When the check itself fails, the feed closes [`CLOSE_RESET`] rather than
//! drop an event without a word.
//!
//! `voice_server_updated` carries a media token: it is forwarded, never
//! logged. Nothing here logs a frame body.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role, WebSocketConfig};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use super::bff::Refusal;
use super::kit_door::{self, call_as, dec, KitCtx, KitPerson, Out, RETRY_SECS};

/// Reconnect: Sew's socket ended, or a check failed. A fresh READY follows.
pub const CLOSE_RESET: u16 = 4000;
/// The K2 login behind the feed ended: sign in again.
pub const CLOSE_LOGIN_ENDED: u16 = 4001;
/// Feeds open at once on one login session.
pub const FEEDS_PER_LOGIN: usize = 3;
/// Ticket, socket, HELLO and READY, start to end.
const HANDSHAKE: Duration = Duration::from_secs(15);
const PING_EVERY: Duration = Duration::from_secs(30);
const VERIFY_TIMEOUT: Duration = Duration::from_secs(10);
/// Channel verdicts kept at most (then K2 starts over).
const VERDICTS_MAX: usize = 10_000;
/// What the phone may send K2 (it sends nothing; pongs and closes are tiny).
const DOWN_MAX: usize = 4 * 1024;

/// The event names that reach the phone (Sew's §2.2 table and its "also
/// forward" list). Everything else is dropped.
pub const EVENTS: &[&str] = &[
    "ready",
    "voice_state_updated",
    "recording_updated",
    "media_policy_updated",
    "call_ring",
    "call_updated",
    "voice_server_updated",
    "perms_changed",
    "install_updated",
    "member_updated",
    "channel_created",
    "channel_updated",
    "channel_member_added",
    "channel_member_removed",
    "space_member_added",
    "space_member_removed",
    "space_updated",
    "category_updated",
    "category_deleted",
];

fn id_of(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// What decides whether one event reaches the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// Forward as Sew sent it.
    Pass,
    Drop,
    /// Forward when the person can see any of these channels.
    AnyOf(Vec<String>),
}

/// The gate for event `t` with body `d`, for the person whose Sew member id
/// is `me` (pure).
pub fn gate(t: &str, d: &Value, me: &str) -> Gate {
    if !EVENTS.contains(&t) {
        return Gate::Drop;
    }
    let chans = |ids: Vec<Option<String>>| {
        let v: Vec<String> = ids.into_iter().flatten().collect();
        if v.is_empty() {
            Gate::Drop
        } else {
            Gate::AnyOf(v)
        }
    };
    match t {
        "voice_state_updated" => {
            if !me.is_empty() && id_of(d.get("member_id")).as_deref() == Some(me) {
                return Gate::Pass;
            }
            chans(vec![id_of(d.get("channel_id")), id_of(d.get("previous_channel_id"))])
        }
        "recording_updated" => chans(vec![id_of(d.pointer("/recording/channel_id"))]),
        "call_updated" => chans(vec![id_of(d.pointer("/call/channel_id"))]),
        "channel_created" | "channel_updated" => chans(vec![id_of(d.get("id"))]),
        _ => Gate::Pass,
    }
}

/// What the feed knows: who the person is in Sew, and which channels they
/// can see.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    /// READY's `member.id`.
    pub me: String,
    verdicts: HashMap<String, bool>,
}

impl Seen {
    /// Seeded from READY: its member, channels, voice channels and calls.
    pub fn from_ready(d: &Value) -> Seen {
        let mut s = Seen { me: id_of(d.pointer("/member/id")).unwrap_or_default(), verdicts: HashMap::new() };
        for (list, key) in [("channels", "id"), ("voice_channels", "id"), ("calls", "channel_id")] {
            for item in d.get(list).and_then(Value::as_array).into_iter().flatten() {
                if let Some(id) = id_of(item.get(key)) {
                    s.verdicts.insert(id, true);
                }
            }
        }
        s
    }

    pub fn known(&self, ch: &str) -> Option<bool> {
        self.verdicts.get(ch).copied()
    }

    fn set(&mut self, ch: &str, visible: bool) {
        if self.verdicts.len() >= VERDICTS_MAX {
            self.verdicts.clear();
        }
        self.verdicts.insert(ch.to_string(), visible);
    }

    /// After event `t`: what it may have changed is asked again.
    pub fn after(&mut self, t: &str) {
        match t {
            "perms_changed" | "channel_member_removed" | "space_member_removed" | "space_updated" => self.verdicts.clear(),
            "channel_created" | "channel_member_added" | "space_member_added" => self.verdicts.retain(|_, v| *v),
            _ => {}
        }
    }
}

// ── limits ───────────────────────────────────────────────────────────

fn open_feeds() -> &'static Mutex<HashMap<String, usize>> {
    static F: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    F.get_or_init(Default::default)
}

struct FeedSlot(String);

impl FeedSlot {
    fn take(sid: &str) -> Option<FeedSlot> {
        let mut g = open_feeds().lock().unwrap_or_else(|e| e.into_inner());
        let n = g.entry(sid.to_string()).or_insert(0);
        if *n >= FEEDS_PER_LOGIN {
            return None;
        }
        *n += 1;
        Some(FeedSlot(sid.to_string()))
    }
}

impl Drop for FeedSlot {
    fn drop(&mut self) {
        let mut g = open_feeds().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = g.get_mut(&self.0) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                g.remove(&self.0);
            }
        }
    }
}

// ── upstream: the person's own Sew session socket ─────────────────────

struct Upstream {
    ws: WebSocketStream<TcpStream>,
    ready: Value,
    ready_s: Option<u64>,
    beat: Duration,
}

fn sew_down(hint: &str) -> Out {
    Out::Refuse(Refusal::new(503, "voice_unavailable", hint).retry(RETRY_SECS))
}

async fn next_frame(ws: &mut WebSocketStream<TcpStream>) -> Option<Value> {
    loop {
        match ws.next().await? {
            Ok(Message::Text(t)) => return serde_json::from_str(&t).ok(),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => continue,
        }
    }
}

async fn open_upstream(ctx: &KitCtx, p: &KitPerson, sew_addr: &str) -> Result<Upstream, Out> {
    // K2's own ticket: safe to ask again on a fresh session after a 401.
    let a = call_as(ctx, p, sew_addr, "POST", "/api/v1/gateway/ticket", Some(b"{}"), &[], true).await.map_err(Out::Refuse)?;
    if a.status != 200 {
        return Err(Out::Sew(a));
    }
    let ticket = serde_json::from_slice::<Value>(&a.body)
        .ok()
        .and_then(|v| v.get("ticket").and_then(Value::as_str).map(str::to_string))
        .filter(|t| !t.is_empty())
        .ok_or_else(|| sew_down("Sew's ticket answer had no ticket"))?;
    // Sew's loopback listener, never the URL in the answer.
    let sa = kit_door::loopback(sew_addr).map_err(|e| sew_down(&e))?;
    let tcp = TcpStream::connect(sa).await.map_err(|_| sew_down("K2 could not reach this server's Sew"))?;
    let (mut ws, _) = tokio_tungstenite::client_async(format!("ws://{sa}/api/v1/gateway?v=1&encoding=json"), tcp)
        .await
        .map_err(|_| sew_down("Sew refused the event socket"))?;
    let hello = next_frame(&mut ws).await.ok_or_else(|| sew_down("Sew closed the event socket"))?;
    if hello.get("op").and_then(Value::as_u64) != Some(8) {
        return Err(sew_down("Sew's event socket didn't say hello"));
    }
    let beat_ms = hello.pointer("/d/heartbeat_interval_ms").and_then(Value::as_u64).unwrap_or(30_000).clamp(1_000, 120_000);
    let identify = json!({
        "op": 2,
        "d": {
            "ticket": ticket,
            "client": { "name": "k2-voice-door", "version": env!("CARGO_PKG_VERSION") },
            "capabilities": 0,
        },
    });
    ws.send(Message::Text(identify.to_string())).await.map_err(|_| sew_down("Sew closed the event socket"))?;
    loop {
        let f = next_frame(&mut ws).await.ok_or_else(|| sew_down("Sew closed the event socket before READY"))?;
        match f.get("op").and_then(Value::as_u64) {
            Some(0) if f.get("t").and_then(Value::as_str) == Some("ready") => {
                let ready_s = f.get("s").and_then(Value::as_u64);
                let ready = f.get("d").cloned().unwrap_or(Value::Null);
                return Ok(Upstream { ws, ready, ready_s, beat: Duration::from_millis(beat_ms) });
            }
            Some(7) => return Err(sew_down("Sew refused the event session")),
            _ => continue,
        }
    }
}

// ── the feed ─────────────────────────────────────────────────────────

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|line| {
        let (n, v) = line.split_once(':')?;
        n.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// Open the person's feed on `client` (the head consumed).
pub(super) async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
    client: &mut S,
    head: &str,
    person: KitPerson,
    bearer: &str,
    ctx: &KitCtx,
    sew_addr: &str,
) {
    let upgrade = header(head, "upgrade").is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    let key = header(head, "sec-websocket-key").filter(|k| !k.is_empty() && k.len() <= 64);
    let (true, Some(key), Some("13")) = (upgrade, key, header(head, "sec-websocket-version")) else {
        return kit_door::refuse(client, &Refusal::new(404, "not_found", "the feed is a WebSocket (Upgrade: websocket, version 13)")).await;
    };
    let Some(_feed) = FeedSlot::take(&person.sid) else {
        let r = Refusal::new(429, "rate_limited", format!("at most {FEEDS_PER_LOGIN} feeds at once on one login")).retry(RETRY_SECS);
        return kit_door::refuse(client, &r).await;
    };
    let Some(_relay) = super::sew_lane::RelaySlot::take() else {
        let r = Refusal::new(503, "voice_unavailable", "too many calls through this server right now").retry(RETRY_SECS);
        return kit_door::refuse(client, &r).await;
    };
    let up = match tokio::time::timeout(HANDSHAKE, open_upstream(ctx, &person, sew_addr)).await {
        Ok(Ok(u)) => u,
        Ok(Err(out)) => return kit_door::write_out(client, &out).await,
        Err(_) => return kit_door::write_out(client, &sew_down("Sew's event socket took too long")).await,
    };
    let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
    let msg = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n{frame}\r\n",
        frame = k2_core::frame_policy::CONNECT_HEADER_LINES,
    );
    if client.write_all(msg.as_bytes()).await.is_err() || client.flush().await.is_err() {
        return;
    }
    let mut cfg = WebSocketConfig::default();
    cfg.max_message_size = Some(DOWN_MAX);
    cfg.max_frame_size = Some(DOWN_MAX);
    let down = WebSocketStream::from_raw_socket(client, Role::Server, Some(cfg)).await;
    run(down, up, &person, bearer, ctx, sew_addr).await;
}

async fn send<S: AsyncRead + AsyncWrite + Unpin>(down: &mut WebSocketStream<S>, s: &mut u64, t: &str, d: Value) -> Result<(), ()> {
    *s += 1;
    let f = json!({ "op": 0, "t": t, "s": *s, "d": d });
    down.send(Message::Text(f.to_string())).await.map_err(|_| ())
}

/// Is channel `ch` one the person can see? Sew answers as them.
async fn verify(ctx: &KitCtx, p: &KitPerson, sew_addr: &str, ch: &str) -> Result<bool, ()> {
    if !dec(ch) {
        return Ok(false);
    }
    let target = format!("/api/v1/channels/{ch}");
    match tokio::time::timeout(VERIFY_TIMEOUT, call_as(ctx, p, sew_addr, "GET", &target, None, &[], true)).await {
        Ok(Ok(a)) if a.status == 200 => Ok(true),
        Ok(Ok(a)) if a.status == 403 || a.status == 404 => Ok(false),
        _ => Err(()),
    }
}

/// Forward event `t`? `Err`: the check failed (the feed resets).
async fn decide(seen: &mut Seen, t: &str, d: &Value, ctx: &KitCtx, p: &KitPerson, sew_addr: &str) -> Result<bool, ()> {
    match gate(t, d, &seen.me) {
        Gate::Pass => Ok(true),
        Gate::Drop => Ok(false),
        Gate::AnyOf(chans) => {
            if chans.iter().any(|c| seen.known(c) == Some(true)) {
                return Ok(true);
            }
            let unknown: Vec<&String> = chans.iter().filter(|c| seen.known(c).is_none()).collect();
            for c in unknown {
                let v = verify(ctx, p, sew_addr, c).await?;
                seen.set(c, v);
                if v {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}

async fn run<S: AsyncRead + AsyncWrite + Unpin>(
    mut down: WebSocketStream<S>,
    up: Upstream,
    person: &KitPerson,
    bearer: &str,
    ctx: &KitCtx,
    sew_addr: &str,
) {
    let Upstream { ws: mut upws, mut ready, ready_s, beat } = up;
    let mut seen = Seen::from_ready(&ready);
    if let Some(o) = ready.as_object_mut() {
        if o.contains_key("resume_url") {
            o.insert("resume_url".into(), json!(""));
        }
    }
    let mut s: u64 = 0;
    let mut last_up = ready_s;
    if send(&mut down, &mut s, "ready", ready).await.is_err() {
        let _ = upws.close(None).await;
        return;
    }
    let now = tokio::time::Instant::now();
    let mut beat_t = tokio::time::interval_at(now + beat, beat);
    let mut ping_t = tokio::time::interval_at(now + PING_EVERY, PING_EVERY);
    let recheck = ctx.deps.recheck.max(Duration::from_millis(10));
    let mut recheck_t = tokio::time::interval_at(now + recheck, recheck);
    let mut acked = true;
    let end: (u16, &'static str) = loop {
        tokio::select! {
            m = upws.next() => match m {
                Some(Ok(Message::Text(t))) => {
                    let Ok(f) = serde_json::from_str::<Value>(&t) else { continue };
                    match f.get("op").and_then(Value::as_u64) {
                        Some(0) => {
                            if let Some(n) = f.get("s").and_then(Value::as_u64) {
                                last_up = Some(n);
                            }
                            let name = f.get("t").and_then(Value::as_str).unwrap_or("").to_string();
                            let d = f.get("d").cloned().unwrap_or(Value::Null);
                            let verdict = decide(&mut seen, &name, &d, ctx, person, sew_addr).await;
                            seen.after(&name);
                            match verdict {
                                Ok(true) => {
                                    if send(&mut down, &mut s, &name, d).await.is_err() {
                                        let _ = upws.close(None).await;
                                        return;
                                    }
                                }
                                Ok(false) => {}
                                Err(()) => break (CLOSE_RESET, "feed_reset"),
                            }
                        }
                        // Sew asks for a beat now.
                        Some(1) => {
                            if upws.send(Message::Text(json!({ "op": 1, "d": last_up }).to_string())).await.is_err() {
                                break (CLOSE_RESET, "feed_reset");
                            }
                        }
                        Some(9) => acked = true,
                        Some(6) | Some(7) => break (CLOSE_RESET, "feed_reset"),
                        _ => {}
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break (CLOSE_RESET, "feed_reset"),
                Some(Ok(_)) => {}
            },
            m = down.next() => match m {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => {
                    let _ = upws.close(None).await;
                    return;
                }
                Some(Ok(_)) => {}
            },
            _ = beat_t.tick() => {
                if !acked {
                    break (CLOSE_RESET, "feed_reset");
                }
                acked = false;
                if upws.send(Message::Text(json!({ "op": 1, "d": last_up }).to_string())).await.is_err() {
                    break (CLOSE_RESET, "feed_reset");
                }
            }
            _ = ping_t.tick() => {
                if down.send(Message::Ping(Vec::new())).await.is_err() {
                    let _ = upws.close(None).await;
                    return;
                }
            }
            _ = recheck_t.tick() => {
                let still = kit_door::resolve(ctx, bearer).await.is_ok_and(|p| p.sid == person.sid);
                if !still {
                    break (CLOSE_LOGIN_ENDED, "login_ended");
                }
            }
        }
    };
    let _ = upws.close(None).await;
    let _ = down.close(Some(CloseFrame { code: CloseCode::from(end.0), reason: end.1.into() })).await;
}
