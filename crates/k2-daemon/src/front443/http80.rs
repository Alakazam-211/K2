//! The box's :80 front (0.46.8, `prd-custom-domains-path-b-0467-v1` PB-8;
//! vs-live PB-A10, PB-A12; Rosson 2026-11-01: on by default once installed,
//! the owner can turn it off).
//!
//! The root helper's `front http install` (protocol 12) has systemd hold
//! `[::]:80` (`k2-front-80.socket`) and pass it to the daemon on every start
//! (`LISTEN_FDS`, `k2-front-80`), like :443's. The daemon answers, per
//! request:
//!
//! - `GET /.well-known/acme-challenge/<token>` for a token the issuer
//!   registered ([`Http01Token`]) → `200` with the key authorization, for
//!   any host. Once the daemon holds :80, K2's own HTTP-01 orders (mail
//!   included) can't bind it per order any more, so they answer here
//!   (PB-A10; `domains::acme`).
//! - a host this box serves at :443 (a `live` custom App name, the mail
//!   host, Skin Direct's host, TURN's) → `308 https://<host><path>`.
//! - anything else → `404`.
//!
//! Path B names never reach this socket: their DNS points at K2's relays,
//! whose own :80 redirects them (relay side, PB-8).
//!
//! The switch is `k2 front http on|off` (owner, an Admin or a shell tab;
//! `FrontConfig::http`). It is on by default: `k2 front enable` installs
//! :80 too where the helper speaks protocol 12 and :80 is free. Off removes
//! the socket (:80 is free again for anything else) and K2's HTTP-01 orders
//! bind :80 per order, as before 0.46.8. The ports view lists `80/tcp`
//! (`front80`) while it serves.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

use k2_core::log_debug;

/// One request's head, at most.
const MAX_HEAD: usize = 8 * 1024;
/// The head must arrive within this.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
const CAP: usize = 1024;
/// `/.well-known/acme-challenge/`.
pub const ACME_PREFIX: &str = "/.well-known/acme-challenge/";

fn tokens() -> &'static Mutex<HashMap<String, String>> {
    static T: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

/// An HTTP-01 answer, served while this guard lives.
pub struct Http01Token {
    token: String,
}

impl Http01Token {
    /// Answer `GET /.well-known/acme-challenge/<token>` with `key_auth`.
    pub fn register(token: &str, key_auth: &str) -> Http01Token {
        tokens().lock().unwrap_or_else(|p| p.into_inner()).insert(token.to_string(), key_auth.to_string());
        Http01Token { token: token.to_string() }
    }
}

impl Drop for Http01Token {
    fn drop(&mut self) {
        tokens().lock().unwrap_or_else(|p| p.into_inner()).remove(&self.token);
    }
}

fn token_answer(token: &str) -> Option<String> {
    tokens().lock().unwrap_or_else(|p| p.into_inner()).get(token).cloned()
}

/// Does this box serve `host` at :443 (so a redirect lands somewhere)?
fn serves(host: &str) -> bool {
    if crate::apps::custom_names::app_for_host(host).is_some() {
        return true;
    }
    super::routes::current().entries().any(|(h, _)| h == host)
}

/// A request's host, lowercase, without its port. `None`: not a host name.
fn host_of(raw: &str) -> Option<String> {
    let h = raw.trim();
    let h = match h.strip_prefix('[') {
        Some(_) => return None, // an IPv6 literal is never one of our names
        None => h.rsplit_once(':').map(|(n, p)| if p.bytes().all(|b| b.is_ascii_digit()) { n } else { h }).unwrap_or(h),
    };
    super::client_hello::host_name(h.trim_end_matches('.').as_bytes())
}

fn response(status: &str, extra: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\n{extra}{frame}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len(),
        frame = k2_core::frame_policy::CONNECT_HEADER_LINES,
    )
}

/// Pure: the answer to one request head (`head` = request line + headers),
/// given what this box serves and the registered tokens.
pub fn answer_with(head: &str, serves: &dyn Fn(&str) -> bool, token: &dyn Fn(&str) -> Option<String>) -> String {
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut parts = first.split(' ');
    let (method, target, version) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if !version.starts_with("HTTP/1.") || !target.starts_with('/') || target.len() > 2048 || parts.next().is_some() {
        return response("400 Bad Request", "", "Bad request\r\n");
    }
    if let Some(t) = target.strip_prefix(ACME_PREFIX) {
        let ok_token = !t.is_empty() && t.len() <= 256 && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if ok_token && (method == "GET" || method == "HEAD") {
            if let Some(k) = token(t) {
                return response("200 OK", "", &k);
            }
        }
        return response("404 Not Found", "", "Not found\r\n");
    }
    let host = lines
        .take_while(|l| !l.is_empty())
        .find_map(|l| l.split_once(':').filter(|(n, _)| n.trim().eq_ignore_ascii_case("host")).map(|(_, v)| v.to_string()))
        .and_then(|h| host_of(&h));
    match host {
        Some(h) if serves(&h) => response("308 Permanent Redirect", &format!("Location: https://{h}{target}\r\n"), ""),
        _ => response("404 Not Found", "", "Not found\r\n"),
    }
}

/// Serve one connection: one request, one answer, then close.
async fn serve_conn(mut tcp: TcpStream) -> Result<(), String> {
    let mut buf = Vec::with_capacity(1024);
    let read = async {
        let mut b = [0u8; 2048];
        while buf.len() < MAX_HEAD && !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = tcp.read(&mut b).await.map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&b[..n]);
        }
        Ok::<(), String>(())
    };
    tokio::time::timeout(HEAD_TIMEOUT, read).await.map_err(|_| "no request head in time".to_string())??;
    let text = String::from_utf8_lossy(&buf);
    let head = text.split("\r\n\r\n").next().unwrap_or("");
    let out = answer_with(head, &serves, &token_answer);
    let _ = tcp.write_all(out.as_bytes()).await;
    let _ = tcp.shutdown().await;
    Ok(())
}

fn cap() -> &'static Arc<Semaphore> {
    static S: OnceLock<Arc<Semaphore>> = OnceLock::new();
    S.get_or_init(|| Arc::new(Semaphore::new(CAP)))
}

struct Live {
    local: Option<SocketAddr>,
    accept: tokio::task::JoinHandle<()>,
}

fn live() -> &'static Mutex<Option<Live>> {
    static L: OnceLock<Mutex<Option<Live>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(None))
}

/// The :80 front holds its socket and answers.
pub fn serving() -> bool {
    live().lock().unwrap_or_else(|p| p.into_inner()).as_ref().is_some_and(|l| !l.accept.is_finished())
}

/// Where it listens (status).
pub fn local_addr() -> Option<SocketAddr> {
    live().lock().unwrap_or_else(|p| p.into_inner()).as_ref().and_then(|l| l.local)
}

async fn accept_loop(l: Arc<TcpListener>) {
    loop {
        let accepted = tokio::select! {
            r = l.accept() => r,
            _ = crate::handoff::quiesced() => return,
        };
        let (tcp, peer) = match accepted {
            Ok(x) => x,
            Err(e) => {
                let fatal = matches!(e.raw_os_error(), Some(9) | Some(22));
                log_debug!("[front80] accept: {e}");
                if fatal {
                    return;
                }
                continue;
            }
        };
        let Ok(permit) = Arc::clone(cap()).try_acquire_owned() else {
            continue;
        };
        // G1: the visitor's open connections, like the App TLS door's.
        let Some(slot) = crate::apps::guard::door_conn(Some(peer.ip())) else { continue };
        tokio::spawn(async move {
            let _permit = permit;
            let _slot = slot;
            if let Err(e) = serve_conn(tcp).await {
                log_debug!("[front80] {peer}: {e}");
            }
        });
    }
}

/// Serve `l` (tests, and [`start`]).
pub fn serve(l: Arc<TcpListener>) {
    let local = l.local_addr().ok();
    let accept = tokio::spawn(accept_loop(l));
    if let Some(old) = live().lock().unwrap_or_else(|p| p.into_inner()).replace(Live { local, accept }) {
        old.accept.abort();
    }
}

/// Boot: serve the :80 socket systemd (or the previous daemon) handed us,
/// while the owner's switch is on. No socket, no :80 front. Never fails boot.
pub async fn start() {
    let std_l = match crate::handoff::take_inherited(crate::handoff::Role::Front80) {
        Some(l) => l,
        None => match super::systemd_fds::take_front80() {
            Some(l) => l,
            None => return,
        },
    };
    if !k2_core::front443::load().http_on() {
        log_debug!("[front80] the :80 front is off (k2 front http off): not serving the socket systemd passed");
        return;
    }
    if let Err(e) = std_l.set_nonblocking(true) {
        log_debug!("[front80] socket set_nonblocking: {e} — :80 front not started");
        return;
    }
    let l = match TcpListener::from_std(std_l) {
        Ok(l) => Arc::new(l),
        Err(e) => {
            log_debug!("[front80] adopt socket: {e} — :80 front not started");
            return;
        }
    };
    crate::handoff::register(crate::handoff::Role::Front80, &l);
    log_debug!("[front80] holding {:?}: https redirect and ACME HTTP-01", l.local_addr().ok());
    serve(l);
}

/// Stop serving (`k2 front http off`, `k2 front disable`): the socket
/// closes with the accept loop.
pub fn stop() -> bool {
    match live().lock().unwrap_or_else(|p| p.into_inner()).take() {
        Some(l) => {
            l.accept.abort();
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serves(h: &str) -> bool {
        h == "shop.example.com" || h == "mail.example.com"
    }
    fn tok(t: &str) -> Option<String> {
        (t == "abc_DEF-123").then(|| "abc_DEF-123.thumb".to_string())
    }

    /// PRD test 12: a live name → 308 to https with its path; an unknown
    /// host → 404; HTTP-01 answered while a token is registered (any host);
    /// a bad request line → 400.
    #[test]
    fn redirects_served_names_answers_tokens_and_404s_the_rest() {
        let a = answer_with("GET /x?y=1 HTTP/1.1\r\nHost: Shop.Example.com:80", &serves, &tok);
        assert!(a.starts_with("HTTP/1.1 308 Permanent Redirect\r\n"), "{a}");
        assert!(a.contains("\r\nLocation: https://shop.example.com/x?y=1\r\n"), "{a}");
        let a = answer_with("POST / HTTP/1.0\r\nhost: mail.example.com", &serves, &tok);
        assert!(a.contains("Location: https://mail.example.com/\r\n"), "{a}");
        let a = answer_with("GET / HTTP/1.1\r\nHost: evil.example", &serves, &tok);
        assert!(a.starts_with("HTTP/1.1 404 "), "{a}");
        let a = answer_with("GET / HTTP/1.1\r\n", &serves, &tok);
        assert!(a.starts_with("HTTP/1.1 404 "), "no host: {a}");
        let a = answer_with("GET / HTTP/1.1\r\nHost: [2001:db8::1]", &serves, &tok);
        assert!(a.starts_with("HTTP/1.1 404 "), "{a}");
        let a = answer_with("GET /.well-known/acme-challenge/abc_DEF-123 HTTP/1.1\r\nHost: anything.example", &serves, &tok);
        assert!(a.starts_with("HTTP/1.1 200 OK\r\n") && a.ends_with("\r\n\r\nabc_DEF-123.thumb"), "{a}");
        let a = answer_with("GET /.well-known/acme-challenge/other HTTP/1.1\r\nHost: shop.example.com", &serves, &tok);
        assert!(a.starts_with("HTTP/1.1 404 "), "an unknown token is never a redirect: {a}");
        let a = answer_with("GET /.well-known/acme-challenge/../x HTTP/1.1\r\nHost: shop.example.com", &serves, &tok);
        assert!(a.starts_with("HTTP/1.1 404 "), "{a}");
        for bad in ["GET http://x/ HTTP/1.1", "GET / FTP/1.0", "GET /a b HTTP/1.1", ""] {
            assert!(answer_with(bad, &serves, &tok).starts_with("HTTP/1.1 400 "), "{bad}");
        }
    }

    /// The registry is the guard's life.
    #[test]
    fn a_token_answers_while_its_guard_lives() {
        let g = Http01Token::register("tok-live-1", "tok-live-1.k");
        assert_eq!(token_answer("tok-live-1").as_deref(), Some("tok-live-1.k"));
        drop(g);
        assert_eq!(token_answer("tok-live-1"), None);
    }

    /// End to end on loopback: a served name gets its redirect.
    #[tokio::test]
    async fn the_socket_answers_one_request_and_closes() {
        let l = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = l.local_addr().unwrap().port();
        let _g = Http01Token::register("tok-e2e-80", "tok-e2e-80.k");
        tokio::spawn(async move {
            let (s, _) = l.accept().await.unwrap();
            serve_conn(s).await.unwrap();
        });
        let mut c = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        c.write_all(b"GET /.well-known/acme-challenge/tok-e2e-80 HTTP/1.1\r\nHost: a.example\r\n\r\n").await.unwrap();
        let mut got = String::new();
        tokio::time::timeout(Duration::from_secs(5), c.read_to_string(&mut got)).await.unwrap().unwrap();
        assert!(got.starts_with("HTTP/1.1 200 OK") && got.ends_with("tok-e2e-80.k"), "{got}");
    }
}
