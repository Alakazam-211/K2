//! The native media-signalling door on an App's own addresses (0.46.9):
//! `wss://<app>.<label>.k2.dev/rtc/v1` (and the App's live custom names)
//! with `Authorization: Bearer <media token>`, spliced to this box's Sew
//! signal listener (the App's approved `sew-signal` box service, on
//! loopback).
//!
//! A native phone App (Tauri plus Sew's plugin) joins a call through its
//! App server, which returns Sew's join answer. The plugin then opens media
//! signalling at the join's `wss://<app host>/rtc/v1`. The App needs no
//! relay of its own for that: K2 answers it here. The App's web kit keeps
//! its own cookie relay, which this door never touches.
//!
//! **Which requests are the door's** ([`classify`]). Only all of these:
//! - `GET`, path exactly `/rtc/v1` (any query), a WebSocket upgrade;
//! - exactly one `Authorization` header, `Bearer <token>`, where the token
//!   is token-shaped (no spaces or control characters, at most 8 KiB);
//! - the token is not one of K2's own (`k2skn_`, `sews_`, `sewcc_`): those
//!   never cross to the engine. Such a request goes to the App as before.
//!
//! Everything else goes to the App exactly as before: `/rtc`, `/rtc/validate`,
//! `/rtc/v1/validate`, a browser's cookie or `?access_token=` query, a plain
//! GET. K2 never reads, checks or trusts the token. The engine is the
//! authority on it.
//!
//! **What crosses.** The request line unchanged (path and full query), the
//! bearer, `Upgrade` / `Connection`, `Sec-WebSocket-Key`, `-Version` and
//! `-Extensions` (permessage-deflate is fine), `User-Agent`, and one
//! `X-Forwarded-For` with the client address K2 derived itself
//! (`crate::client_ip`, through K2 Connect's relays and tunnel hops). Every
//! other header is dropped, so a client's own `X-Forwarded-For`,
//! `Forwarded`, `X-Real-IP`, cookie, Origin or subprotocol never reaches the
//! engine. The engine's default `trusted_proxies` lists loopback, so it
//! takes the address from K2's header and its per-address join limit sees
//! each phone, not K2. With no client address known, K2 sends none.
//!
//! **Limits.** No idle timeout and no message cap of K2's own on the
//! upgraded socket (the engine pings every few seconds and caps client
//! messages itself). No per-token socket rule: two sockets on one identity
//! (a reconnect overlapping a resume) both pass. A cap on sockets at once,
//! separate from the call door's.
//!
//! The token and the query string (`join_request`, `access_token`) are never
//! logged, and never appear in an answer K2 writes.

use std::net::IpAddr;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};

use super::sew_lane::{self, RelaySlot};

/// The one path this door answers.
pub const PATH: &str = "/rtc/v1";
/// Longest token K2 passes on.
const MAX_TOKEN: usize = 8 * 1024;
/// K2's own bearer prefixes: an App pass, a Sew session, a call credential.
pub const K2_PREFIXES: &[&str] = &["k2skn_", "sews_", "sewcc_"];
/// The headers that cross besides the bearer, the upgrade and K2's XFF.
const FORWARD: &[&str] = &["sec-websocket-key", "sec-websocket-version", "sec-websocket-extensions", "user-agent"];

/// What a request head on `/rtc*` is to this door.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Class {
    /// The native engine's socket: K2 splices it (when the App's gate is open).
    Native,
    /// The App's, unchanged (a browser's cookie or query token, another
    /// path, a K2 credential, no upgrade).
    App,
}

/// Every `name:` header value in `head` (the request line skipped).
fn headers<'a>(head: &'a str, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    head.lines().skip(1).filter_map(move |line| {
        let (n, v) = line.split_once(':')?;
        n.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// The token of a `Bearer` value when it is token-shaped.
fn bearer_of(value: &str) -> Option<&str> {
    let (scheme, token) = value.split_once(char::is_whitespace)?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    let shaped = !token.is_empty() && token.len() <= MAX_TOKEN && token.bytes().all(|b| b.is_ascii_graphic());
    shaped.then_some(token)
}

/// Is `head` (a complete request head) the native engine's signalling
/// socket? Pure: K2 never parses the token.
pub fn classify(head: &str) -> Class {
    let mut first = head.lines().next().unwrap_or("").split_whitespace();
    let (Some(method), Some(target)) = (first.next(), first.next()) else {
        return Class::App;
    };
    let path = target.split_once('?').map_or(target, |(p, _)| p);
    if method != "GET" || path != PATH || !sew_lane::wants_upgrade(head) {
        return Class::App;
    }
    let mut auth = headers(head, "authorization");
    let (Some(value), None) = (auth.next(), auth.next()) else {
        return Class::App;
    };
    match bearer_of(value) {
        Some(token) if !K2_PREFIXES.iter().any(|p| token.starts_with(p)) => Class::Native,
        _ => Class::App,
    }
}

/// The head at the start of `bytes` when it is complete and the door's:
/// `(head text, head length in bytes with its blank line)`.
pub fn native_head(bytes: &[u8]) -> Option<(String, usize)> {
    let len = bytes.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let head = String::from_utf8_lossy(&bytes[..len]).into_owned();
    (classify(&head) == Class::Native).then_some((head, len))
}

/// The request K2 sends the engine. `None` when the head isn't the door's.
pub fn upstream_head(head: &str, signal_addr: &str, client: Option<IpAddr>) -> Option<String> {
    if classify(head) != Class::Native {
        return None;
    }
    let target = head.lines().next()?.split_whitespace().nth(1)?;
    let token = bearer_of(headers(head, "authorization").next()?)?;
    let mut out = format!("GET {target} HTTP/1.1\r\nHost: {signal_addr}\r\nAuthorization: Bearer {token}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n");
    for name in FORWARD {
        // A control character in a value is never ours to pass on.
        if let Some(v) = headers(head, name).next().filter(|v| !v.chars().any(char::is_control)) {
            out.push_str(&format!("{name}: {v}\r\n"));
        }
    }
    if let Some(ip) = client {
        out.push_str(&format!("X-Forwarded-For: {ip}\r\n"));
    }
    out.push_str("\r\n");
    Some(out)
}

/// A log line about a request on this door: the method and the path only,
/// never the query or a header.
pub fn log_line(what: &str, head: &str) -> String {
    let mut first = head.lines().next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("?");
    let path = first.next().map_or("?", |t| t.split_once('?').map_or(t, |(p, _)| p));
    format!("[voice/signal-lane] {what}: {method} {path}")
}

/// Splice one native signalling socket whose head is still unread on
/// `client` (only peeked; [`native_head`] said it is the door's) to the
/// engine at `signal_addr` (loopback only). The connection closes afterwards.
pub async fn serve<S: AsyncRead + AsyncWrite + Unpin>(client: &mut S, head: &str, head_len: usize, signal_addr: &str, visitor: Option<IpAddr>) {
    let mut consumed = vec![0u8; head_len];
    if client.read_exact(&mut consumed).await.is_err() {
        return;
    }
    let Some(out) = upstream_head(head, signal_addr, visitor) else {
        return sew_lane::answer(client, "400 Bad Request", "usage", "not a media signalling request", None).await;
    };
    let Some(_slot) = RelaySlot::take_signal() else {
        k2_core::log_debug!("{}", log_line("too many signalling sockets", head));
        return sew_lane::answer(client, "503 Service Unavailable", "voice_unavailable", "too many calls through this server right now", Some(2)).await;
    };
    sew_lane::relay_gateway(client, signal_addr, out.as_bytes()).await;
}

#[cfg(test)]
#[path = "signal_lane_tests.rs"]
mod tests;
