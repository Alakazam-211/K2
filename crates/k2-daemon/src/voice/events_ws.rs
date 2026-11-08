//! `WS /cli/voice/events` (prd-voice-chat-only-members-v1 §6.3, D6): the
//! voice rooms' own socket.
//!
//! Sends only `{"kind":"hello"}` once, then `{"kind":"voice_rooms_changed",
//! "rev":n}` from the same publisher the app socket carries
//! (`voice::occupancy::emit_rooms_changed`), plus WebSocket pings. Nothing
//! else from the session bus ever goes out here: no workspace, session or
//! person frame. Any live login may use it (a Voice & Chat only login is the
//! reason it exists: it never gets `/cli/sessions/events`), and a new client
//! uses it for every server that isn't its window's own, which before got
//! only a 60 s sweep (V21). The owner token too. Dark (404 `voice_rooms_off`,
//! answered by the dispatcher) while the preview is off.
//!
//! The login is asked again before every frame when the login store moved
//! and on the 5 s tick (`socket_access`), at this route's `VoiceChat` floor:
//! a removed, disabled or signed-out login closes with 4003.

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::broadcast::error::RecvError;
use tokio_tungstenite::tungstenite::Message;

use crate::routes::route_policy::Floor;
use crate::session_events::{self, SessionEvent};

pub const PATH: &str = "/cli/voice/events";

/// The most of these sockets one login may hold open (security review H2).
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

/// The cap is for Voice & Chat only logins (re-review L9): the owner, Admins
/// and Members open as many as their windows need. `Ok(None)` = not counted,
/// `Ok(Some)` = counted, `Err(())` = a Voice & Chat only login already holds
/// [`MAX_PER_LOGIN`].
#[allow(clippy::result_unit_err)]
pub fn slot_for(login: &str, role: k2_core::connect_users::Role) -> Result<Option<Slot>, ()> {
    if role != k2_core::connect_users::Role::VoiceChat {
        return Ok(None);
    }
    take_slot(login).map(Some).ok_or(())
}

/// The one frame this socket forwards, as JSON (None for every other event).
pub fn frame_for(event: &SessionEvent) -> Option<String> {
    match event {
        SessionEvent::VoiceRoomsChanged { rev } => {
            Some(serde_json::json!({ "kind": "voice_rooms_changed", "rev": rev }).to_string())
        }
        _ => None,
    }
}

/// Serve one upgraded connection. `token` already passed the dispatcher's
/// door (`token_ok` and the route gate).
pub async fn serve(stream: &mut TcpStream, token: String, owner_token: String) {
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            k2_core::log_debug!("[voice/events] handshake failed: {e}");
            return;
        }
    };
    let (mut write, mut read) = ws.split();
    let mut rx = session_events::subscribe();
    let still = |t: &str, o: &str| crate::routes::http::login_still_admitted(t, o, Floor::VoiceChat);
    let mut seen = k2_core::connect_users::auth_generation();
    if write.send(Message::Text(serde_json::json!({ "kind": "hello" }).to_string().into())).await.is_err() {
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
                if write.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
            recv = rx.recv() => {
                match recv {
                    Ok(event) => {
                        let Some(frame) = frame_for(&event) else { continue };
                        let now = k2_core::connect_users::auth_generation();
                        if now != seen {
                            if !still(&token, &owner_token) {
                                let _ = write.send(crate::presence::session_revoked_close_message()).await;
                                break;
                            }
                            seen = now;
                        }
                        if write.send(Message::Text(frame.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(_)) => {
                        // Fell behind: one change frame is enough to re-read.
                        let frame = serde_json::json!({ "kind": "voice_rooms_changed", "rev": 0 }).to_string();
                        if write.send(Message::Text(frame.into())).await.is_err() {
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
    fn a_login_holds_at_most_three_sockets_and_a_closed_one_frees_its_place() {
        let a = take_slot("slotcap").expect("1");
        let b = take_slot("slotcap").expect("2");
        let c = take_slot("slotcap").expect("3");
        assert!(take_slot("slotcap").is_none(), "a fourth is refused");
        assert!(take_slot("slotother").is_some(), "another login is separate");
        drop(b);
        let d = take_slot("slotcap").expect("a place came free");
        assert!(take_slot("slotcap").is_none());
        drop((a, c, d));
    }

    #[test]
    fn only_voice_chat_logins_are_capped() {
        use k2_core::connect_users::Role;
        let member: Vec<_> = (0..6).map(|i| slot_for("slotmember", Role::Member).unwrap_or_else(|_| panic!("Member socket {i} refused"))).collect();
        assert!(member.iter().all(Option::is_none), "a Member's sockets aren't counted");
        for role in [Role::Admin, Role::Owner] {
            for _ in 0..5 {
                assert!(matches!(slot_for("slotadmin", role), Ok(None)), "{role:?} isn't capped");
            }
        }
        let held: Vec<_> = (0..MAX_PER_LOGIN).map(|_| slot_for("slotvc", Role::VoiceChat).expect("under the cap").expect("counted")).collect();
        assert!(slot_for("slotvc", Role::VoiceChat).is_err(), "a fourth Voice & Chat only socket is refused");
        drop(held);
    }

    #[test]
    fn only_rooms_changed_frames_leave() {
        assert_eq!(
            frame_for(&SessionEvent::VoiceRoomsChanged { rev: 7 }).as_deref(),
            Some(r#"{"kind":"voice_rooms_changed","rev":7}"#)
        );
        assert_eq!(frame_for(&SessionEvent::DomainsChanged { reason: "x".into(), apex: None }), None);
    }
}
