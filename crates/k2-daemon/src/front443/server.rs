//! The front's accept loop and per-connection splice.
//!
//! One connection: take a permit from the front's own cap (or close at
//! once), peek the ClientHello within [`HELLO_TIMEOUT`] and
//! [`super::client_hello::MAX_HELLO_BYTES`], look the server name up in the
//! route table, dial the loopback backend, write the PROXY v2 header (when
//! the route wants one) and every byte read so far in ONE write, then
//! splice both ways. TLS is never terminated here: the backend sees the
//! client's own ClientHello. A missing SNI, an unknown name, a non-TLS
//! stream or a bad ClientHello is closed without a byte written.
//!
//! Connections run on the hand-off splice runtime (they outlive a
//! no-downtime restart: the old daemon drains them) and hold a splice
//! guard for their whole life.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

use k2_core::log_debug;

use super::client_hello::{peek, Peek};
use super::proxy_v2;
use super::routes::{Kind, RouteTable};

/// Whole-ClientHello deadline (a slowloris that dribbles a byte a second
/// is cut here).
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// Loopback backend dial deadline.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The front's cap bounds: [`k2_core::connection_cap`] of the fd budget.
pub const CAP_FLOOR: usize = 256;
pub const CAP_CEILING: usize = 16_384;
/// Most connections one source address may hold while still sending its
/// ClientHello: a slow-hello flood from one address can't take the whole
/// cap (TURN, Skin and mail share it).
pub const HELLO_PER_IP: u32 = 32;

/// Per-outcome counters (since boot; a successor starts at zero).
#[derive(Default)]
pub struct Counters {
    pub accepted: AtomicU64,
    pub over_cap: AtomicU64,
    pub live: AtomicU64,
    pub turn: AtomicU64,
    pub caddy: AtomicU64,
    pub stalwart: AtomicU64,
    /// Custom App names routed to the App TLS door (0.46.6).
    pub app: AtomicU64,
    /// Owner passthrough names (0.46.6, `k2 front passthrough`).
    pub passthrough: AtomicU64,
    pub unknown_sni: AtomicU64,
    pub no_sni: AtomicU64,
    pub not_tls: AtomicU64,
    pub bad_hello: AtomicU64,
    pub hello_timeout: AtomicU64,
    pub closed_early: AtomicU64,
    pub loopback_refused: AtomicU64,
    pub backend_failed: AtomicU64,
    pub per_ip_refused: AtomicU64,
}

impl Counters {
    fn bump(c: &AtomicU64) {
        c.fetch_add(1, Ordering::Relaxed);
    }
    pub fn json(&self) -> Value {
        let g = |c: &AtomicU64| c.load(Ordering::Relaxed);
        json!({
            "accepted": g(&self.accepted),
            "overCap": g(&self.over_cap),
            "perIpRefused": g(&self.per_ip_refused),
            "live": g(&self.live),
            "routed": {"turn": g(&self.turn), "caddy": g(&self.caddy), "stalwart": g(&self.stalwart), "app": g(&self.app), "passthrough": g(&self.passthrough)},
            "closed": {
                "unknownSni": g(&self.unknown_sni),
                "noSni": g(&self.no_sni),
                "notTls": g(&self.not_tls),
                "badHello": g(&self.bad_hello),
                "helloTimeout": g(&self.hello_timeout),
                "closedEarly": g(&self.closed_early),
                "loopbackRefused": g(&self.loopback_refused),
                "backendFailed": g(&self.backend_failed),
            },
        })
    }
}

type TableFn = dyn Fn() -> Arc<RouteTable> + Send + Sync;

/// One front: its cap, deadlines, counters and where it reads routes.
pub struct Front {
    pub cap: usize,
    pub hello_timeout: Duration,
    pub connect_timeout: Duration,
    pub hello_per_ip: u32,
    sem: Arc<Semaphore>,
    pub counters: Counters,
    table: Box<TableFn>,
    /// Connections still in the ClientHello phase, per source address.
    hellos: Mutex<HashMap<IpAddr, u32>>,
}

/// One connection's place in its source's ClientHello budget.
struct HelloSlot {
    front: Arc<Front>,
    ip: IpAddr,
}

impl Drop for HelloSlot {
    fn drop(&mut self) {
        let mut m = self.front.hellos.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(n) = m.get_mut(&self.ip) {
            *n -= 1;
            if *n == 0 {
                m.remove(&self.ip);
            }
        }
    }
}

impl Front {
    fn hello_slot(self: &Arc<Self>, ip: IpAddr) -> Option<HelloSlot> {
        let mut m = self.hellos.lock().unwrap_or_else(|p| p.into_inner());
        let n = m.entry(ip).or_insert(0);
        if *n >= self.hello_per_ip {
            return None;
        }
        *n += 1;
        Some(HelloSlot { front: Arc::clone(self), ip })
    }
}

impl Front {
    pub fn new(cap: usize, table: Box<TableFn>) -> Arc<Front> {
        Arc::new(Front {
            cap,
            hello_timeout: HELLO_TIMEOUT,
            connect_timeout: CONNECT_TIMEOUT,
            hello_per_ip: HELLO_PER_IP,
            sem: Arc::new(Semaphore::new(cap)),
            counters: Counters::default(),
            table,
            hellos: Mutex::new(HashMap::new()),
        })
    }

    /// Test knob: shorter deadlines.
    pub fn with_timeouts(cap: usize, hello: Duration, connect: Duration, table: Box<TableFn>) -> Arc<Front> {
        Self::with_limits(cap, hello, connect, HELLO_PER_IP, table)
    }

    /// Test knob: deadlines and the per-source ClientHello budget.
    pub fn with_limits(cap: usize, hello: Duration, connect: Duration, hello_per_ip: u32, table: Box<TableFn>) -> Arc<Front> {
        Arc::new(Front {
            cap,
            hello_timeout: hello,
            connect_timeout: connect,
            hello_per_ip,
            sem: Arc::new(Semaphore::new(cap)),
            counters: Counters::default(),
            table,
            hellos: Mutex::new(HashMap::new()),
        })
    }
}

/// Accept until the listener fails or the daemon quiesces for a hand-off
/// (the socket stays open: the hand-off passes it on). Connections are
/// spawned on `rt`.
pub async fn accept_loop(listener: Arc<TcpListener>, front: Arc<Front>, rt: tokio::runtime::Handle) {
    loop {
        let accepted = tokio::select! {
            r = listener.accept() => r,
            _ = crate::handoff::quiesced() => {
                log_debug!("[front443] accept loop stopped for the hand-off");
                return;
            }
        };
        let (tcp, peer) = match accepted {
            Ok(x) => x,
            Err(e) => {
                let fatal = matches!(e.raw_os_error(), Some(libc::EBADF) | Some(libc::EINVAL) | Some(libc::ENOTSOCK));
                if fatal {
                    log_debug!("[front443] fatal accept error — accept loop ends: {e}");
                    return;
                }
                // EMFILE/ENFILE and friends: back off a little, keep going.
                log_debug!("[front443] accept error: {e}");
                tokio::time::sleep(Duration::from_millis(20)).await;
                continue;
            }
        };
        Counters::bump(&front.counters.accepted);
        let Ok(permit) = Arc::clone(&front.sem).try_acquire_owned() else {
            Counters::bump(&front.counters.over_cap);
            drop(tcp);
            continue;
        };
        let Some(slot) = front.hello_slot(proxy_v2::canonical(peer.ip())) else {
            Counters::bump(&front.counters.per_ip_refused);
            drop(tcp);
            continue;
        };
        let std_tcp = match tcp.into_std() {
            Ok(s) => s,
            Err(e) => {
                log_debug!("[front443] detach accepted socket: {e}");
                continue;
            }
        };
        let f = Arc::clone(&front);
        rt.spawn(async move {
            let _permit = permit;
            let _splice = crate::handoff::splice_guard();
            f.counters.live.fetch_add(1, Ordering::Relaxed);
            if let Ok(tcp) = TcpStream::from_std(std_tcp) {
                serve(&f, tcp, peer, slot).await;
            }
            f.counters.live.fetch_sub(1, Ordering::Relaxed);
        });
    }
}

/// Where the complete TLS records of `buf` end, walked on from `from`
/// (a record boundary). `None` when a header can't be a handshake record
/// (the full peek then says why). Lets [`read_hello`] re-parse only when a
/// record completed, not after every byte of a dribbled hello.
fn complete_records(buf: &[u8], from: usize) -> Option<usize> {
    if buf.first().is_some_and(|b| *b != 0x16) {
        return None;
    }
    let mut pos = from;
    while buf.len() >= pos + 5 {
        let h = &buf[pos..pos + 5];
        let len = usize::from(u16::from_be_bytes([h[3], h[4]]));
        if h[0] != 0x16 || len == 0 || len > 16 * 1024 {
            return None;
        }
        if buf.len() < pos + 5 + len {
            break;
        }
        pos += 5 + len;
    }
    Some(pos)
}

/// Read until the ClientHello is complete. `None` = closed (counted).
async fn read_hello(f: &Front, tcp: &mut TcpStream) -> Option<(Vec<u8>, super::client_hello::Hello)> {
    let deadline = tokio::time::Instant::now() + f.hello_timeout;
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let mut walked = 0usize; // complete records found so far
    let mut peeked = 0usize; // ... when the full parse last ran
    loop {
        let full = match complete_records(&buf, walked) {
            Some(end) => {
                walked = end;
                end > peeked || buf.len() >= super::client_hello::MAX_HELLO_BYTES
            }
            None => true,
        };
        let verdict = if full {
            peeked = walked;
            peek(&buf)
        } else {
            Peek::NeedMore
        };
        match verdict {
            Peek::Done(h) => return Some((buf, h)),
            Peek::NotTls => {
                Counters::bump(&f.counters.not_tls);
                return None;
            }
            Peek::Bad(_) => {
                Counters::bump(&f.counters.bad_hello);
                return None;
            }
            Peek::NeedMore => {}
        }
        match tokio::time::timeout_at(deadline, tcp.read(&mut chunk)).await {
            Err(_) => {
                Counters::bump(&f.counters.hello_timeout);
                return None;
            }
            Ok(Ok(0)) | Ok(Err(_)) => {
                Counters::bump(&f.counters.closed_early);
                return None;
            }
            Ok(Ok(n)) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

async fn serve(f: &Front, mut tcp: TcpStream, peer: SocketAddr, slot: HelloSlot) {
    let hello = read_hello(f, &mut tcp).await;
    drop(slot);
    let Some((head, hello)) = hello else {
        return;
    };
    let table = (f.table)();
    // No SNI: closed, unless the owner set a default passthrough (their own
    // web server answers it, as it did when it held :443 itself).
    let (sni, b) = match hello.sni {
        Some(sni) => match table.lookup(&sni).cloned() {
            Some(b) => (sni, b),
            None => {
                Counters::bump(&f.counters.unknown_sni);
                return;
            }
        },
        None => match table.default_route().cloned() {
            Some(b) => ("(no SNI)".to_string(), b),
            None => {
                Counters::bump(&f.counters.no_sni);
                return;
            }
        },
    };
    let client_ip = proxy_v2::canonical(peer.ip());
    if b.refuse_loopback_clients && client_ip.is_loopback() {
        Counters::bump(&f.counters.loopback_refused);
        return;
    }
    let mut up = match tokio::time::timeout(f.connect_timeout, TcpStream::connect(b.addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            Counters::bump(&f.counters.backend_failed);
            log_debug!("[front443] {sni}: backend {} ({}) refused: {e}", b.addr, b.kind.as_str());
            return;
        }
        Err(_) => {
            Counters::bump(&f.counters.backend_failed);
            log_debug!("[front443] {sni}: backend {} ({}) dial timed out", b.addr, b.kind.as_str());
            return;
        }
    };
    let _ = up.set_nodelay(true);
    let _ = tcp.set_nodelay(true);
    let mut first = Vec::with_capacity(head.len() + 52);
    if b.proxy {
        let local = tcp.local_addr().unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 443)));
        first.extend_from_slice(&proxy_v2::header(peer, local));
    }
    first.extend_from_slice(&head);
    if up.write_all(&first).await.is_err() {
        Counters::bump(&f.counters.backend_failed);
        return;
    }
    match b.kind {
        Kind::Turn => Counters::bump(&f.counters.turn),
        Kind::Caddy => Counters::bump(&f.counters.caddy),
        Kind::Stalwart => Counters::bump(&f.counters.stalwart),
        Kind::App => Counters::bump(&f.counters.app),
        Kind::Passthrough => Counters::bump(&f.counters.passthrough),
    }
    let _ = tokio::io::copy_bidirectional(&mut tcp, &mut up).await;
}
