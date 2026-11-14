//! `WS /cli/home-chat/events` (§14.4): one person's Home chat frames, from
//! their follower's broadcast. Frames: `hello` first, then `message`,
//! `deleted`, `read`, `reaction`, `typing`, `member_removed`,
//! `summary_changed`, `status` and forwarded `event`s (`call_ring`,
//! `call_updated`). Nothing from any other person or server.
//!
//! The login is asked again before a frame when the login store moved, and
//! on the 5 s tick (`socket_access`), at the `VoiceChat` floor: a removed,
//! disabled or signed-out login closes with 4003. At most [`MAX_PER_LOGIN`]
//! open per login (HP-11).

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::broadcast::error::RecvError;
use tokio_tungstenite::tungstenite::Message;

use crate::routes::route_policy::Floor;

/// The most of these sockets one login may hold open.
pub const MAX_PER_LOGIN: usize = 3;

fn open_counts() -> &'static std::sync::Mutex<std::collections::HashMap<String, usize>> {
    static M: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, usize>>> = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

/// One open socket's place in its login's count; freed on drop.
#[derive(Debug)]
pub struct Slot(String);

impl Drop for Slot {
    fn drop(&mut self) {
        let mut m = open_counts().lock().unwrap_or_else(|p| p.into_inner());
        if let Some(n) = m.get_mut(&self.0) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                m.remove(&self.0);
            }
        }
    }
}

/// A place for one more socket of `login`, or `None` at [`MAX_PER_LOGIN`].
pub fn take_slot(login: &str) -> Option<Slot> {
    let mut m = open_counts().lock().unwrap_or_else(|p| p.into_inner());
    let n = m.entry(login.to_string()).or_insert(0);
    if *n >= MAX_PER_LOGIN {
        return None;
    }
    *n += 1;
    Some(Slot(login.to_string()))
}

/// Serve one upgraded connection for person `ext` (the dispatcher already
/// checked the login and took the slot).
pub async fn serve(stream: &mut TcpStream, ext: String, token: String, owner_token: String) {
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            k2_core::log_debug!("[home-chat/events] handshake failed: {e}");
            return;
        }
    };
    let (mut write, mut read) = ws.split();
    let held = super::follower::hold(&ext);
    let mut rx = held.events.subscribe();
    let still = |t: &str, o: &str| crate::routes::http::login_still_admitted(t, o, Floor::VoiceChat);
    let mut seen = k2_core::connect_users::auth_generation();
    let hello = serde_json::json!({"type": "hello", "chat": held.status(), "flags": (super::deps().flags)()});
    if write.send(Message::Text(hello.to_string())).await.is_err() {
        return;
    }
    let mut tick = crate::socket_access::tick();
    let mut ping = tokio::time::interval(std::time::Duration::from_secs(25));
    ping.tick().await;
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if !still(&token, &owner_token) {
                    let _ = write.send(crate::presence::session_revoked_close_message()).await;
                    break;
                }
            }
            _ = ping.tick() => {
                if write.send(Message::Ping(Vec::new())).await.is_err() {
                    break;
                }
            }
            recv = rx.recv() => {
                match recv {
                    Ok(frame) => {
                        let now = k2_core::connect_users::auth_generation();
                        if now != seen {
                            if !still(&token, &owner_token) {
                                let _ = write.send(crate::presence::session_revoked_close_message()).await;
                                break;
                            }
                            seen = now;
                        }
                        if write.send(Message::Text(frame)).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(_)) => {
                        // Fell behind: one summary frame is enough to re-read.
                        let f = serde_json::json!({"type": "summary_changed"}).to_string();
                        if write.send(Message::Text(f)).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Closed) => break,
                }
            }
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Ping(p))) => {
                        if write.send(Message::Pong(p)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        let _ = write.send(Message::Close(frame)).await;
                        break;
                    }
                    Some(Ok(_)) => {}
                    None | Some(Err(_)) => break,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_holds_at_most_three_sockets() {
        let a = take_slot("hc-slot").expect("1");
        let b = take_slot("hc-slot").expect("2");
        let c = take_slot("hc-slot").expect("3");
        assert!(take_slot("hc-slot").is_none(), "a fourth is refused");
        drop(b);
        let d = take_slot("hc-slot").expect("a place came free");
        drop((a, c, d));
    }
}
