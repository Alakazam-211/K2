//! The daemon-local App route override and drain (§7.2 step 7, AR4).
//!
//! Router targets for `*.<sub>.k2.dev` come from the control plane
//! (`SubdomainMap`), and a target change is a control-plane write. A deploy
//! never makes one. Instead:
//! - The control-plane target of an App label is the App's **front port**:
//!   a stable loopback listener the daemon owns (`apps.front_port`).
//! - This module holds the **override**: App label → the live instance's
//!   backend port, in an `ArcSwap` read once per connection. The tunnel TLS
//!   listener (`serve_one`) consults it before dialing the control-plane
//!   target, and the front listener uses it for every connection, so local
//!   (`--no-tunnel`) access sees the same switch.
//! - A switch only changes where **new** connections go. Spliced
//!   connections to the old backend keep running until they close or the
//!   drain deadline passes; then they are cut.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use arc_swap::ArcSwap;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::watch;

/// One instance's backend: its port, the connections spliced to it, and
/// the signal that cuts them at the end of a drain.
#[derive(Debug)]
pub struct Backend {
    pub app_id: String,
    pub port: u16,
    /// 0.46.3: rewrite each HTTP/1.1 request for the client IP
    /// (`crate::client_ip`). `false` = the manifest's `host.clientIp:
    /// false` escape hatch: a raw splice, as before.
    pub client_ip: bool,
    active: AtomicUsize,
    total: AtomicUsize,
    draining: AtomicBool,
    cut_tx: watch::Sender<bool>,
}

impl Backend {
    fn new(app_id: &str, port: u16, client_ip: bool) -> Arc<Self> {
        let (cut_tx, _) = watch::channel(false);
        Arc::new(Self {
            app_id: app_id.to_string(),
            port,
            client_ip,
            active: AtomicUsize::new(0),
            total: AtomicUsize::new(0),
            draining: AtomicBool::new(false),
            cut_tx,
        })
    }

    /// Connections spliced to this backend right now.
    pub fn active(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }

    /// Connections ever spliced to this backend.
    pub fn total(&self) -> usize {
        self.total.load(Ordering::SeqCst)
    }

    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }
}

type Table = HashMap<String, Arc<Backend>>;

fn table() -> &'static ArcSwap<Table> {
    static T: OnceLock<ArcSwap<Table>> = OnceLock::new();
    T.get_or_init(|| ArcSwap::from_pointee(HashMap::new()))
}

fn write_lock() -> &'static Mutex<()> {
    static L: Mutex<()> = Mutex::new(());
    &L
}

/// The live backend for an App label (one atomic load).
pub fn live(app_id: &str) -> Option<Arc<Backend>> {
    table().load().get(app_id).cloned()
}

/// Point `app_id` at `port`. Returns the backend that was live before
/// (the caller drains it). Never a control-plane call. The client-IP
/// rewrite is on ([`switch_with`] sets it from the manifest).
pub fn switch(app_id: &str, port: u16) -> (Arc<Backend>, Option<Arc<Backend>>) {
    switch_with(app_id, port, true)
}

/// [`switch`] with the App's `host.clientIp` (`false` = raw splice).
pub fn switch_with(app_id: &str, port: u16, client_ip: bool) -> (Arc<Backend>, Option<Arc<Backend>>) {
    let _g = write_lock().lock().unwrap_or_else(|e| e.into_inner());
    let cur = table().load_full();
    let prev = cur.get(app_id).cloned();
    if let Some(p) = &prev {
        if p.port == port && p.client_ip == client_ip {
            return (p.clone(), None);
        }
    }
    let be = Backend::new(app_id, port, client_ip);
    let mut next: Table = (*cur).clone();
    next.insert(app_id.to_string(), be.clone());
    table().store(Arc::new(next));
    (be, prev)
}

/// Remove an App's route (stop / uninstall). Returns the old backend.
pub fn clear(app_id: &str) -> Option<Arc<Backend>> {
    let _g = write_lock().lock().unwrap_or_else(|e| e.into_inner());
    let cur = table().load_full();
    let prev = cur.get(app_id).cloned()?;
    let mut next: Table = (*cur).clone();
    next.remove(app_id);
    table().store(Arc::new(next));
    Some(prev)
}

/// Let `be`'s spliced connections finish, cutting what is left after
/// `deadline`. Returns how many were cut. Blocking (call from a thread).
pub fn drain_blocking(be: &Arc<Backend>, deadline: Duration) -> usize {
    be.draining.store(true, Ordering::SeqCst);
    let end = std::time::Instant::now() + deadline;
    while be.active() > 0 && std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
    }
    let left = be.active();
    if left > 0 {
        let _ = be.cut_tx.send(true);
        // Give the splices a moment to see the cut.
        let grace = std::time::Instant::now() + Duration::from_secs(2);
        while be.active() > 0 && std::time::Instant::now() < grace {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    left
}

struct Guard(Arc<Backend>);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Splice `client` to `be`, counted for drain and cut at its deadline.
/// `Some(mode)`: every request is rewritten for it (`crate::client_ip`:
/// client copies of the forwarding headers dropped, `X-K2-Client-IP` added
/// when K2 knows the client). `None`: the raw splice (before 0.46.3, and
/// `host.clientIp: false`). Pick with [`rewrite_mode`]. `Err` only when the
/// backend can't be dialed (the caller answers 502).
pub async fn splice<S>(client: &mut S, be: Arc<Backend>, mode: Option<crate::client_ip::Mode>) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    splice_with(client, be, mode, None).await
}

/// [`splice`] with K2's own answers for some requests on the rewrite path
/// (`local`, the App's pairing link files; ignored for a raw splice).
pub async fn splice_with<S>(
    client: &mut S,
    be: Arc<Backend>,
    mode: Option<crate::client_ip::Mode>,
    local: Option<crate::client_ip::LocalAnswer>,
) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut upstream = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(("127.0.0.1", be.port)))
        .await
        .map_err(|_| format!("App {} backend :{} did not accept within 5s", be.app_id, be.port))?
        .map_err(|e| format!("App {} backend :{}: {e}", be.app_id, be.port))?;
    // 0.46.3: never splice visitors into the daemon's own main listener;
    // any other splice is marked so it can never pass as local.
    crate::splice_guard::refuse_self_dial(&upstream)
        .map_err(|why| format!("App {} backend :{}: {why}", be.app_id, be.port))?;
    let _splice_mark = crate::splice_guard::mark(&upstream);
    be.active.fetch_add(1, Ordering::SeqCst);
    be.total.fetch_add(1, Ordering::SeqCst);
    let _guard = Guard(be.clone());
    let mut cut = be.cut_tx.subscribe();
    if *cut.borrow() {
        return Ok(());
    }
    if let Some(mode) = mode {
        tokio::select! {
            _ = crate::client_ip::forward_with(&mut *client, &mut upstream, mode, local) => {}
            _ = cut.changed() => {}
        }
    } else {
        // `host.clientIp: false`: the pre-0.46.3 raw splice, unchanged.
        tokio::select! {
            _ = tokio::io::copy_bidirectional(client, &mut upstream) => {}
            _ = cut.changed() => {}
        }
    }
    let _ = upstream.shutdown().await;
    Ok(())
}

/// The rewrite for `be` reached as `label`: on when the App's manifest
/// allows it (`host.clientIp`, default on), or when the label opted in with
/// `k2 publish client-ip <label> on` (a `--cmd` App starts raw).
pub fn rewrite_mode(be: &Backend, label: &str, mode: crate::client_ip::Mode) -> Option<crate::client_ip::Mode> {
    (be.client_ip || k2_core::tunnel::client_ip_labels::enabled(label)).then_some(mode)
}

const UNAVAILABLE_BODY: &str = "This App is not running now.\r\n";
const BAD_GATEWAY_BODY: &str = "This App did not answer K2.\r\n";

/// K2's own answer on an App's front port (no backend, or it didn't
/// answer). It carries the daemon's framing headers like every other
/// response K2 writes (prd-app-frame-ancestors-v1 FA6, routes/frame_walk).
fn own_answer(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nCache-Control: no-store\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n{body}",
        body.len(),
        k2_core::frame_policy::CONNECT_HEADER_LINES,
    )
}

/// What [`answer_first`] did with a connection's first request.
pub enum FirstRequest {
    /// K2 answered it (the connection is done).
    Answered,
    /// The App answers: these bytes were read and must be replayed.
    Pass(Vec<u8>),
}

/// Read a connection's first request head **before** the App is dialed
/// and answer it when `local` does (an App's pairing link files, so they
/// work while the App is down: app pairing V5).
pub async fn answer_first<S>(client: &mut S, local: &crate::client_ip::LocalAnswer) -> FirstRequest
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let p = crate::client_ip::peek_head(client).await;
    if let Some((method, path, host)) = &p.plain {
        if let Some(bytes) = local(method, path, host.as_deref()) {
            let _ = client.write_all(&bytes).await;
            let _ = client.shutdown().await;
            return FirstRequest::Answered;
        }
    }
    FirstRequest::Pass(p.bytes)
}

/// Serve one connection for `app_id`: splice to its live backend, 503
/// when it has none, 502 when the backend doesn't answer. `peer` is the
/// front port's caller (loopback): K2's own tunnel hop keeps its client
/// IP; any other caller only loses `X-K2-Client-IP`.
///
/// While the App's pairing is on (`app_pairing::links`), the first
/// request is read before the backend is dialed and K2 answers the link
/// files itself, App up or down; later requests on the connection get the
/// same answers in order (`client_ip::forward_with`).
pub async fn serve_app_conn<S>(client: &mut S, app_id: &str, peer: Option<std::net::SocketAddr>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let local = crate::app_pairing::links::local_for(app_id);
    match live(app_id) {
        None => {
            match &local {
                Some(l) => {
                    if let FirstRequest::Answered = answer_first(client, l).await {
                        return;
                    }
                }
                None => consume_request_head(client).await,
            }
            let _ = client.write_all(own_answer("503 Service Unavailable", UNAVAILABLE_BODY).as_bytes()).await;
        }
        Some(be) => {
            let mode = rewrite_mode(&be, app_id, crate::client_ip::Mode::Local { peer });
            match (mode, local) {
                (Some(mode), Some(l)) => match answer_first(client, &l).await {
                    FirstRequest::Answered => return,
                    FirstRequest::Pass(bytes) => {
                        let failed = {
                            let mut r = crate::client_ip::Replay::new(bytes, client);
                            splice_with(&mut r, be, Some(mode), Some(l)).await.is_err()
                        };
                        if failed {
                            let _ = client.write_all(own_answer("502 Bad Gateway", BAD_GATEWAY_BODY).as_bytes()).await;
                        }
                    }
                },
                (mode, _) => {
                    if splice(client, be, mode).await.is_err() {
                        consume_request_head(client).await;
                        let _ = client.write_all(own_answer("502 Bad Gateway", BAD_GATEWAY_BODY).as_bytes()).await;
                    }
                }
            }
        }
    }
    let _ = client.shutdown().await;
}

/// Read the request head before answering an error ourselves: closing a
/// socket with unread bytes makes the kernel send a reset, and the client
/// would see "connection reset" instead of our 503/502.
async fn consume_request_head<S>(client: &mut S)
where
    S: AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut got = Vec::new();
    let mut b = [0u8; 2048];
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        while got.len() < 16 * 1024 && !got.windows(4).any(|w| w == b"\r\n\r\n") {
            match client.read(&mut b).await {
                Ok(0) | Err(_) => break,
                Ok(n) => got.extend_from_slice(&b[..n]),
            }
        }
    })
    .await;
}

// ── front listeners ──────────────────────────────────────────────────

struct Front {
    port: u16,
    task: tokio::task::JoinHandle<()>,
}

fn fronts() -> &'static Mutex<HashMap<String, Front>> {
    static F: OnceLock<Mutex<HashMap<String, Front>>> = OnceLock::new();
    F.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `ensure_front` couldn't bind the stored port (the hand-off's
/// predecessor still holds it, R36): the caller retries, never moves it.
pub const FRONT_PORT_HELD: &str = "front_port_held";

/// Make sure `app_id`'s front listener runs on `want` (0 = any free
/// port). Returns the port it listens on (a different one when `want` was
/// taken and `fallback` allows it; the caller records it). Needs a tokio
/// runtime handle.
///
/// Update continuity (0.46.4): a listener the hand-off's predecessor
/// passed over is taken first (R19); without `fallback` a taken port is
/// [`FRONT_PORT_HELD`], never a random one (R36). The accept loop stops at
/// a hand-off's quiesce (R37: the socket stays open and the successor takes
/// it), and every front connection holds a splice guard so the old daemon
/// drains it before exiting (`handoff::app_conn_guard`).
pub fn ensure_front(handle: &tokio::runtime::Handle, app_id: &str, want: u16, fallback: bool) -> Result<u16, String> {
    let mut f = fronts().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(cur) = f.get(app_id) {
        if !cur.task.is_finished() && (want == 0 || cur.port == want) {
            return Ok(cur.port);
        }
        cur.task.abort();
        f.remove(app_id);
    }
    let std_l = match crate::handoff::take_inherited_app(app_id, crate::handoff::AppKind::Front) {
        Some(l) => l,
        None => {
            let bound = match std::net::TcpListener::bind(("127.0.0.1", want)) {
                Ok(l) => Ok(l),
                Err(e) if want != 0 && e.kind() == std::io::ErrorKind::AddrInUse && !fallback => {
                    return Err(format!("{FRONT_PORT_HELD}: 127.0.0.1:{want} ({e})"));
                }
                Err(e) if want != 0 => std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|_| e),
                Err(e) => Err(e),
            };
            bound.map_err(|e| format!("bind the App's front port: {e}"))?
        }
    };
    let port = std_l.local_addr().map_err(|e| e.to_string())?.port();
    std_l.set_nonblocking(true).map_err(|e| e.to_string())?;
    let app = app_id.to_string();
    let _rt = handle.enter();
    let l = std::sync::Arc::new(tokio::net::TcpListener::from_std(std_l).map_err(|e| e.to_string())?);
    crate::handoff::register_app(app_id, crate::handoff::AppKind::Front, &l);
    let task = handle.spawn(async move {
        loop {
            tokio::select! {
                a = l.accept() => {
                    let Ok((mut s, peer)) = a else { continue };
                    let app = app.clone();
                    let guard = crate::handoff::app_conn_guard();
                    tokio::spawn(async move {
                        serve_app_conn(&mut s, &app, Some(peer)).await;
                        drop(guard);
                    });
                }
                _ = crate::handoff::quiesced() => break,
            }
        }
    });
    f.insert(app_id.to_string(), Front { port, task });
    Ok(port)
}

/// Stop `app_id`'s front listener (uninstall).
pub fn close_front(app_id: &str) {
    if let Some(f) = fronts().lock().unwrap_or_else(|e| e.into_inner()).remove(app_id) {
        f.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    async fn tagged_server(tag: &'static str, hold_ms: u64) -> u16 {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = l.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut b = [0u8; 512];
                    let _ = s.read(&mut b).await;
                    tokio::time::sleep(Duration::from_millis(hold_ms)).await;
                    let body = format!("{tag}\n");
                    let _ = s
                        .write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes())
                        .await;
                    let _ = s.shutdown().await;
                });
            }
        });
        port
    }

    async fn get_via_front(front: u16) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", front)).await.unwrap();
        s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
        // A cut connection may end in a reset: keep what arrived.
        let mut out = Vec::new();
        let mut b = [0u8; 4096];
        loop {
            match s.read(&mut b).await {
                Ok(0) | Err(_) => break,
                Ok(n) => out.extend_from_slice(&b[..n]),
            }
        }
        String::from_utf8_lossy(&out).to_string()
    }

    /// 0.46.3: `host.clientIp: false` keeps the raw splice (no rewrite, no
    /// 400 for non-HTTP bytes); the default rewrites. Changing only the
    /// flag makes a new backend.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn client_ip_flag_picks_rewrite_or_raw() {
        // The opt-in list lives under $HOME: sandbox it.
        let _home = crate::test_support::TempHome::new();
        // A backend that echoes whatever it first reads.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = l.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut b = [0u8; 2048];
                    let n = s.read(&mut b).await.unwrap_or(0);
                    let r = format!("HTTP/1.1 200 OK\r\nContent-Length: {n}\r\n\r\n");
                    let _ = s.write_all(r.as_bytes()).await;
                    let _ = s.write_all(&b[..n]).await;
                    let _ = s.shutdown().await;
                });
            }
        });
        let app = "router-client-ip-app";
        let front = ensure_front(&tokio::runtime::Handle::current(), app, 0, true).unwrap();
        let ask = |bytes: &'static [u8]| async move {
            let mut s = TcpStream::connect(("127.0.0.1", front)).await.unwrap();
            s.write_all(bytes).await.unwrap();
            let mut out = Vec::new();
            let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await.expect("answer");
            String::from_utf8_lossy(&out).to_string()
        };
        let spoof: &[u8] = b"GET / HTTP/1.1\r\nHost: x\r\nX-K2-Client-IP: 6.6.6.6\r\n\r\n";

        let (be, _) = switch(app, port);
        assert!(be.client_ip, "switch = the default: rewrite on");
        let got = ask(spoof).await;
        assert!(got.contains("200 OK") && !got.contains("6.6.6.6"), "rewrite drops the client copy: {got}");
        assert!(ask(b"\x00\x01binary\r\n\r\n").await.starts_with("HTTP/1.1 400"), "rewrite refuses non-HTTP");

        let (raw, prev) = switch_with(app, port, false);
        assert!(!raw.client_ip && prev.is_some(), "a flag change is a new backend");
        assert!(ask(spoof).await.contains("X-K2-Client-IP: 6.6.6.6"), "raw splice: bytes unchanged");
        let got = ask(b"\x00\x01binary\r\n\r\n").await;
        assert!(got.starts_with("HTTP/1.1 200") && got.contains("binary"), "raw: any protocol passes: {got}");

        // A raw App (a `--cmd` one) can still opt in by label.
        k2_core::tunnel::client_ip_labels::set(app, true).unwrap();
        let got = ask(spoof).await;
        assert!(got.contains("200 OK") && !got.contains("6.6.6.6"), "opted in by label: rewritten: {got}");

        clear(app);
        close_front(app);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn switch_moves_new_connections_and_old_ones_drain() {
        let app = "router-test-app";
        let a = tagged_server("A", 0).await;
        let a_slow = tagged_server("A-slow", 800).await;
        let b = tagged_server("B", 0).await;
        let front = ensure_front(&tokio::runtime::Handle::current(), app, 0, true).unwrap();
        assert!(get_via_front(front).await.contains("503"), "no live backend yet");

        let (_, prev) = switch(app, a);
        assert!(prev.is_none());
        assert!(get_via_front(front).await.ends_with("A\n"));

        // An in-flight request on the old backend survives the switch.
        let (old_slow, _) = switch(app, a_slow);
        let inflight = tokio::spawn(get_via_front(front));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(old_slow.active(), 1);
        let (_, prev) = switch(app, b);
        let prev = prev.expect("previous backend");
        assert_eq!(prev.port, a_slow);
        assert!(get_via_front(front).await.ends_with("B\n"), "new connections go to the new backend");
        let p2 = prev.clone();
        let cut = tokio::task::spawn_blocking(move || drain_blocking(&p2, Duration::from_secs(5))).await.unwrap();
        assert_eq!(cut, 0, "it finished inside the drain");
        assert!(inflight.await.unwrap().ends_with("A-slow\n"), "the old request completed");

        // Past the deadline, what is left is cut.
        let slow2 = tagged_server("never", 10_000).await;
        let (old, _) = switch(app, slow2);
        let stuck = tokio::spawn(get_via_front(front));
        tokio::time::sleep(Duration::from_millis(200)).await;
        switch(app, b);
        let o2 = old.clone();
        let cut = tokio::task::spawn_blocking(move || drain_blocking(&o2, Duration::from_millis(300))).await.unwrap();
        assert_eq!(cut, 1);
        let out = tokio::time::timeout(Duration::from_secs(5), stuck).await.expect("cut, not hung").unwrap();
        assert!(!out.contains("never"));
        assert_eq!(old.active(), 0);

        assert!(clear(app).is_some());
        assert!(get_via_front(front).await.contains("503"));
        close_front(app);
    }
}

/// R36 + R19 (update continuity, 0.46.4): the front listener a hand-off
/// passed over is taken first (same socket, same port); a stored port held
/// by someone else is never moved without `fallback` (the predecessor still
/// drains), and moves only with it (any other boot).
#[cfg(test)]
mod handoff_tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_held_front_port_is_kept_and_an_inherited_one_is_taken_first() {
        let h = tokio::runtime::Handle::current();
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = held.local_addr().unwrap().port();
        let e = ensure_front(&h, "r36-held", port, false).unwrap_err();
        assert!(e.starts_with(FRONT_PORT_HELD), "{e}");
        let moved = ensure_front(&h, "r36-held", port, true).unwrap();
        assert_ne!(moved, port, "outside a hand-off a taken port still falls back");
        close_front("r36-held");

        // The predecessor's own socket arrives over the hand-off: taken
        // first, so the stored port is served with no bind at all (the
        // `held` socket above is exactly such a socket, still bound).
        crate::handoff::inherit_app_for_test("r19-front", crate::handoff::AppKind::Front, held);
        let got = ensure_front(&h, "r19-front", port, false).unwrap();
        assert_eq!(got, port, "the stored port, through the inherited socket");
        let mut c = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        use tokio::io::AsyncReadExt;
        let mut b = Vec::new();
        let _ = c.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").await;
        let _ = tokio::time::timeout(Duration::from_secs(5), c.read_to_end(&mut b)).await;
        assert!(String::from_utf8_lossy(&b).starts_with("HTTP/1.1 503"), "served by this daemon's router: {}", String::from_utf8_lossy(&b));
        close_front("r19-front");
    }
}
