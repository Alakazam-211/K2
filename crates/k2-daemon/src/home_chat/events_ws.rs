//! `WS /cli/home-chat/events` (§14.4): one person's Home chat frames, from
//! their follower's broadcast. Frames: `hello` first, then `message`,
//! `deleted`, `read`, `reaction`, `typing`, `member_removed`,
//! `summary_changed`, `status`, `message_updated` (a call's record moved)
//! and the DM call frames `call_ring` and `call_updated` (`hello` carries
//! the calls already ringing), and `notify {conv, level, effective,
//! inherited, muted}` when the person's level changed on any device
//! (prd-home-chat-pushes-v1 §7). Nothing from any other person or server.
//!
//! **Viewing leases** (prd-home-chat-pushes-v1 HC-15): the one thing a client
//! sends, `{"t":"viewing","conv":"<sew id>","visible":true|false}`, repeated
//! every 30 s while a conversation is open, visible and focused. Each is
//! kept for 90 s under this socket ([`super::chat_push::lease`]); `false`, or
//! the socket closing, clears it. Leases only quiet phone pushes.
//!
//! The login is asked again before a frame when the login store moved, and
//! on the 5 s tick (`socket_access`), at the `VoiceChat` floor: a removed,
//! disabled or signed-out login closes with 4003. At most [`MAX_PER_LOGIN`]
//! open per login (HP-11).
//!
//! **Access** (`login_access_live`): when what this socket's own login may do
//! changes (a share revoked or changed, its role, its chat grant), it gets
//! `{"type":"access_changed"}` and reads its access again. A Voice & Chat
//! only phone may hold only this socket. Never another login's, and never
//! the owner token's (its access doesn't change).

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::broadcast::error::RecvError;
use tokio_tungstenite::tungstenite::Message;

use crate::routes::route_policy::Floor;
use crate::session_events::SessionEvent;

/// This socket's access frame, in its own `type` envelope.
pub const ACCESS_CHANGED: &str = r#"{"type":"access_changed"}"#;

/// The access frame owed for one session bus event, on a socket held by
/// `login` (`None`: the owner token): only `access_changed` naming that
/// very login.
pub fn access_frame(event: &SessionEvent, login: Option<&str>) -> Option<&'static str> {
    match event {
        SessionEvent::AccessChanged { .. } if crate::session_events_ws::login_may_see(event, login) => Some(ACCESS_CHANGED),
        _ => None,
    }
}

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
    let socket = crate::thread_viewers::next_socket_id();
    serve_ws(ws, &ext, &token, &owner_token, socket).await;
    // HC-15: a closed (or crashed) client's leases go with its socket.
    super::chat_push::close_socket(socket);
}

async fn serve_ws(ws: tokio_tungstenite::WebSocketStream<&mut TcpStream>, ext: &str, token: &str, owner_token: &str, socket: u64) {
    let (ext, token, owner_token) = (ext.to_string(), token.to_string(), owner_token.to_string());
    let (mut write, mut read) = ws.split();
    let held = super::follower::hold(&ext);
    let mut rx = held.events.subscribe();
    let still = |t: &str, o: &str| crate::routes::http::login_still_admitted(t, o, Floor::VoiceChat);
    let mut seen = k2_core::connect_users::auth_generation();
    // Whose `access_changed` this socket carries: its own login's only. Taken
    // before the hello, so a change after the client's first read is heard.
    let mut access_rx = crate::session_events::subscribe();
    let login = crate::session_events_ws::socket_login(crate::presence::resolve_identity(&token, &owner_token).as_ref());
    let _access_hold = match login.clone() {
        Some(l) => match tokio::task::spawn_blocking(move || crate::login_access_live::hold(&l)).await {
            Ok(held) => held,
            Err(e) => {
                k2_core::log_debug!("[home-chat/events] access hold failed: {e}");
                None
            }
        },
        None => None,
    };
    let calls = held.state.lock().unwrap_or_else(|p| p.into_inner()).calls_view();
    let hello = serde_json::json!({"type": "hello", "chat": held.status(), "flags": (super::deps().flags)(), "calls": calls});
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
            event = access_rx.recv() => {
                let frame = match event {
                    Ok(event) => access_frame(&event, login.as_deref()),
                    // Fell behind the session bus: the access frame may be
                    // among what was lost, and one more read costs little.
                    Err(RecvError::Lagged(_)) => login.is_some().then_some(ACCESS_CHANGED),
                    Err(RecvError::Closed) => break,
                };
                let Some(frame) = frame else { continue };
                if !still(&token, &owner_token) {
                    let _ = write.send(crate::presence::session_revoked_close_message()).await;
                    break;
                }
                seen = k2_core::connect_users::auth_generation();
                if write.send(Message::Text(frame.to_string())).await.is_err() {
                    break;
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
                    Some(Ok(Message::Text(t))) => {
                        if let Some((conv, visible)) = super::chat_push::parse_viewing(&t) {
                            super::chat_push::lease(socket, &ext, &conv, visible);
                        }
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
    fn only_the_login_s_own_access_change_is_a_frame() {
        let ev = SessionEvent::AccessChanged { login: "hcgina".into() };
        assert_eq!(access_frame(&ev, Some("hcgina")), Some(r#"{"type":"access_changed"}"#));
        assert_eq!(access_frame(&ev, Some("hcbob")), None, "another login's");
        assert_eq!(access_frame(&ev, None), None, "the owner token");
        assert_eq!(access_frame(&SessionEvent::ProjectsChanged {}, Some("hcgina")), None, "nothing else from the session bus");
    }

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
