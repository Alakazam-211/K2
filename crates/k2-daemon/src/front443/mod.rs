//! front443 — the box's :443 as an SNI pass-through router inside the
//! daemon (0.46.1; plan `plan-turn-443-front-v1`, Rosson 2026-10-17).
//!
//! The daemon holds no capability. The root helper's `front install`
//! (protocol 8) has systemd listen on `[::]:443` (`k2-front-443.socket`)
//! and pass the socket to the daemon's unit on every start
//! (`LISTEN_FDS`, picked up in [`systemd_fds`]); a no-downtime hand-off
//! passes it on as `Role::Front443`.
//!
//! The front never terminates TLS. For each connection it peeks the
//! ClientHello's server name ([`client_hello`]), looks it up in the route
//! table ([`routes`]), connects to that loopback backend, writes a PROXY
//! v2 header ([`proxy_v2`]) and then the client's raw bytes, ClientHello
//! first, and splices ([`server`]). Unknown names, no SNI, non-TLS and
//! malformed hellos are closed. Routes come from the daemon's own state
//! ([`rebuild`]):
//!
//! - `turn-<label>.k2.dev` → sew-media's `passthrough` TURN listener on
//!   127.0.0.1:5349 (PROXY v2 always; Sew's contract, TURN-PT rev 168);
//! - Skin Direct's host (and the mail host while Caddy fronts mail) →
//!   Caddy on 127.0.0.1:38473 (PROXY v2 only on Caddy ≥ 2.7);
//! - opt-in, the mail host and its extra names → Stalwart's HTTPS on
//!   127.0.0.1:8443 with `overrideProxyTrustedNetworks` (PROXY v2; loopback
//!   clients refused, so no failed login through the front lands on 127.0.0.1).
//!
//! - 0.46.6, owner passthrough: names the owner's own web server answers
//!   (a system Caddy or nginx moved from :443 to a loopback port) →
//!   that port, untouched, no PROXY unless asked; `*` = every name nothing
//!   else routes ([`passthrough_add`], `k2 front passthrough`). K2's own
//!   names always win.
//!
//! Owner surface: `GET /cli/front/status`, `POST /cli/front/enable`
//! (`{"mail": bool}`), `POST /cli/front/disable`, `GET
//! /cli/front/passthrough`, `POST /cli/front/passthrough/{add,rm}`; CLI
//! `k2 front …`. `status` and `enable` name a web server that isn't K2's
//! holding :443 ([`port443_holder`]) with the ways to fix it: K2 never
//! fights for the port.

pub mod client_hello;
pub mod proxy_v2;
pub mod routes;
pub mod server;
pub mod systemd_fds;

#[cfg(test)]
mod tests;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};

use k2_core::log_debug;
use k2_root_proto::version::FRONT_MIN_PROTOCOL;

use routes::{Backend, Kind, RouteTable};

/// sew-media's passthrough TURN listener (Sew's default).
pub const TURN_BACKEND: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 5349);
/// Stalwart's HTTPS listener behind the front.
pub const STALWART_BACKEND: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 8443);
/// How often the route table is rebuilt from the daemon's state (on top of
/// the rebuilds the enable / disable / voice paths do themselves).
const REFRESH: Duration = Duration::from_secs(30);

struct Live {
    front: Arc<server::Front>,
    local: Option<SocketAddr>,
    source: &'static str,
    accept: tokio::task::JoinHandle<()>,
}

fn live() -> &'static Mutex<Option<Live>> {
    static L: OnceLock<Mutex<Option<Live>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(None))
}

/// The front holds :443 and its accept loop runs.
pub fn serving() -> bool {
    live().lock().unwrap_or_else(|p| p.into_inner()).as_ref().is_some_and(|l| !l.accept.is_finished())
}

/// The front's connection cap for this process (from `RLIMIT_NOFILE`).
pub fn cap() -> usize {
    static CAP: OnceLock<usize> = OnceLock::new();
    *CAP.get_or_init(|| k2_core::connection_cap(server::CAP_FLOOR, server::CAP_CEILING))
}

/// Boot: serve the :443 socket systemd (or the previous daemon) handed us.
/// No socket, no front. Never fails boot.
pub async fn start() {
    let (std_l, source) = match crate::handoff::take_inherited(crate::handoff::Role::Front443) {
        Some(l) => (l, "handoff"),
        None => match systemd_fds::take_front() {
            Some(l) => (l, "systemd"),
            None => return,
        },
    };
    if let Err(e) = std_l.set_nonblocking(true) {
        log_debug!("[front443] socket set_nonblocking: {e} — front not started");
        return;
    }
    let l = match tokio::net::TcpListener::from_std(std_l) {
        Ok(l) => Arc::new(l),
        Err(e) => {
            log_debug!("[front443] adopt socket: {e} — front not started");
            return;
        }
    };
    crate::handoff::register(crate::handoff::Role::Front443, &l);
    // 0.46.6: the App TLS door (custom App names) serves while the front
    // does; a hand-off successor takes its socket over (Role::AppDoor).
    crate::apps::tls_door::start();
    let local = l.local_addr().ok();
    // A hand-off successor routes from its first connection with the
    // table its predecessor passed (fixed backends only). A cold start
    // (systemd's fd) accepts only once its own table is built — the DB is
    // open a few seconds in — and the kernel queues connections meanwhile
    // (backlog 4096), so nothing that arrives during boot is closed for a
    // route that simply isn't loaded yet.
    let handed = routes::take_handed();
    let (ready_tx, mut ready_rx) = tokio::sync::watch::channel(handed.is_some());
    if let Some(mut t) = handed {
        // The owner's passthrough names never ride the hand-off: add them
        // from the owner's file (no DB needed), so they route at once too.
        let mut w = Vec::new();
        add_passthrough(&mut t, &mut w);
        for e in w {
            log_debug!("[front443] passthrough not added: {e}");
        }
        routes::install(t);
    }
    let front = server::Front::new(cap(), Box::new(routes::current));
    let accept = {
        let front = Arc::clone(&front);
        tokio::spawn(async move {
            tokio::select! {
                r = ready_rx.wait_for(|r| *r) => { if r.is_err() { return; } }
                _ = crate::handoff::quiesced() => return,
            }
            server::accept_loop(l, front, crate::handoff::splice_handle()).await
        })
    };
    *live().lock().unwrap_or_else(|p| p.into_inner()) = Some(Live { front, local, source, accept });
    let held = local.map(|a| a.to_string()).unwrap_or_else(|| "nothing (no local address)".to_string());
    log_debug!("[front443] holding {held} (socket from {source}; cap {})", cap());
    // Routes now, then on a timer; converge voice onto the passthrough tier.
    tokio::spawn(async move {
        // The live table needs the DB (hosted mail's names): wait for boot
        // to open it, at most a minute.
        for _ in 0..120 {
            if k2_core::db::try_shared().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let mut first = true;
        loop {
            let _ = tokio::task::spawn_blocking(rebuild_logged).await;
            if first {
                let _ = ready_tx.send(true);
                log_debug!("[front443] serving {local:?}");
                // Sew's order: the front accepts and routes before TURN moves.
                let _ = tokio::task::spawn_blocking(|| converge_voice(true)).await;
                first = false;
            }
            if !serving() {
                return;
            }
            tokio::time::sleep(REFRESH).await;
        }
    });
}

/// Stop serving (`disable`): the accept loop ends and its socket closes;
/// live splices finish on their own.
pub fn stop() -> bool {
    let l = live().lock().unwrap_or_else(|p| p.into_inner()).take();
    match l {
        Some(l) => {
            l.accept.abort();
            routes::install(RouteTable::default());
            true
        }
        None => false,
    }
}

/// Build the route table from the daemon's state and swap it in.
/// Returns the table and anything that could not be routed.
pub fn rebuild() -> (Arc<RouteTable>, Vec<String>) {
    let cfg = k2_core::front443::load();
    let mut t = RouteTable::default();
    let mut warnings = Vec::new();
    if cfg.enabled {
        let mut add = |host: &str, b: Backend| {
            if let Err(e) = t.insert(host, b) {
                warnings.push(e);
            }
        };
        // Stalwart before Caddy: with mail behind the front, the mail name
        // is Stalwart's (Caddy then renders no mail site anyway).
        turn_routes(&mut add);
        stalwart_routes(&cfg, &mut add);
        caddy_routes(&mut add);
        // Last: a custom App name never takes a name TURN, mail or Skin
        // Direct already routes (first route wins; refused at bind too).
        app_routes(&mut add);
        // After every K2 name: the owner's passthrough names (K2's win).
        add_passthrough(&mut t, &mut warnings);
    }
    routes::install(t);
    (routes::current(), warnings)
}

/// The owner's passthrough names (`~/.k2/front443-passthrough.json`) into
/// `t`, after K2's own: a name K2 already routes is refused (warned), `*`
/// becomes the default route.
pub fn add_passthrough(t: &mut RouteTable, warnings: &mut Vec<String>) {
    let list = match k2_core::front443::load_passthrough_checked() {
        Ok(v) => v,
        Err(e) => {
            warnings.push(format!("passthrough names not loaded: {e}"));
            return;
        }
    };
    for p in list {
        let b = Backend { kind: Kind::Passthrough, addr: p.to, proxy: p.proxy_v2, refuse_loopback_clients: false };
        let r = if p.host == k2_core::front443::PASSTHROUGH_DEFAULT { t.set_default(b) } else { t.insert(&p.host, b) };
        if let Err(e) = r {
            warnings.push(format!("passthrough {}: {e}", p.host));
        }
    }
}

/// 0.46.6: every custom App name the door serves (`live`) or is getting a
/// certificate for (`issuing`, so the CA's TLS-ALPN-01 hello reaches it)
/// → the App TLS door on 127.0.0.1:38474, PROXY v2 on.
fn app_routes(add: &mut Add) {
    for h in crate::apps::tls_door::reload().routed {
        add(&h, routes::fixed_backend(Kind::App));
    }
}

/// What `host` is already used for at :443 on this box (the mail host and
/// its extra names, Skin Direct's host, the TURN name), `None` = free. A
/// custom App name equal to one of them is refused at bind (PRD §8.4).
pub fn current_use(host: &str) -> Option<String> {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if mail_names().iter().any(|m| m.eq_ignore_ascii_case(&h)) {
        return Some(format!("{h} is this box's mail host (hosted mail)"));
    }
    if let Some(c) = k2_core::skin_door::front_routes() {
        if c.hosts.iter().any(|x| x.eq_ignore_ascii_case(&h)) {
            return Some(format!("{h} is Skin Direct's host on this box"));
        }
    }
    if k2_core::skin::front_door_is_stored() {
        let stored = k2_core::skin::get_front_door().ok().and_then(|fd| fd.url).map(|u| {
            let rest = u.split_once("://").map(|(_, r)| r.to_string()).unwrap_or(u);
            rest.split(['/', ':']).next().unwrap_or("").trim_end_matches('.').to_ascii_lowercase()
        });
        if stored.as_deref() == Some(h.as_str()) {
            return Some(format!("{h} is Skin Direct's host on this box"));
        }
    }
    for (rh, b) in routes::current().entries() {
        if rh == &h && b.kind == Kind::Passthrough {
            return Some(format!("{h} is passed through to {} on :443 (k2 front passthrough rm {h})", b.addr));
        }
        if rh == &h && b.kind != Kind::App {
            return Some(format!("{h} already routes to {} on :443", b.kind.as_str()));
        }
    }
    None
}

/// Rebuild now (a custom name was bound, unbound or changed state). With
/// the front off only the door's own table is refreshed (the route table
/// is already empty; `disable` cleared it).
pub fn rebuild_now() {
    if !k2_core::front443::load().enabled {
        crate::apps::tls_door::reload();
        return;
    }
    rebuild_logged();
}

type Add<'a> = dyn FnMut(&str, Backend) + 'a;

/// S3: `turn-<label>.k2.dev` → sew-media's passthrough listener, once
/// voice runs on that tier (PROXY v2 always: Sew's contract).
fn turn_routes(add: &mut Add) {
    let host = crate::sew::voice::front_route_host(&crate::sew::supervisor::live())
        .or_else(|| TURN_PENDING.lock().unwrap_or_else(|p| p.into_inner()).clone());
    if let Some(h) = host {
        add(&h, routes::fixed_backend(Kind::Turn));
    }
}

/// The TURN name routed BEFORE voice moves to the passthrough tier (Sew's
/// order: the front must route the TURN SNI before sew-media leaves public
/// 5349), cleared once the move is done or abandoned.
static TURN_PENDING: Mutex<Option<String>> = Mutex::new(None);

fn set_turn_pending(h: Option<String>) {
    *TURN_PENDING.lock().unwrap_or_else(|p| p.into_inner()) = h;
    rebuild_logged();
}

/// The live front and the loopback address of its socket.
fn live_front() -> Option<(Arc<server::Front>, SocketAddr)> {
    let g = live().lock().unwrap_or_else(|p| p.into_inner());
    let l = g.as_ref().filter(|l| !l.accept.is_finished())?;
    let port = l.local?.port();
    Some((Arc::clone(&l.front), SocketAddr::from(([127, 0, 0, 1], port))))
}

/// Prove the front routes `host` end to end: a ClientHello for it through
/// the front's own socket must reach the TURN backend (the front peeked the
/// name, found the route, dialled 127.0.0.1:5349 and wrote PROXY v2 + the
/// hello; the counter moves only then). Blocking, a few seconds at most.
pub fn probe_route(host: &str) -> Result<(), String> {
    let (front, addr) = live_front().ok_or("the :443 front is not serving")?;
    probe_through(&front, addr, host)
}

/// [`probe_route`] against a given front and its address (tests).
pub fn probe_through(front: &server::Front, addr: SocketAddr, host: &str) -> Result<(), String> {
    use std::io::{Read, Write};
    let before = front.counters.turn.load(Ordering::Relaxed);
    let failed_before = front.counters.backend_failed.load(Ordering::Relaxed);
    let mut c = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| format!("connect {addr}: {e}"))?;
    let _ = c.set_read_timeout(Some(Duration::from_secs(3)));
    c.write_all(&client_hello::synthetic(host)).map_err(|e| format!("write the probe hello: {e}"))?;
    let mut sink = [0u8; 512];
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while front.counters.turn.load(Ordering::Relaxed) == before && std::time::Instant::now() < deadline {
        match c.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
    for _ in 0..30 {
        if front.counters.turn.load(Ordering::Relaxed) > before {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if front.counters.backend_failed.load(Ordering::Relaxed) > failed_before {
        return Err(format!("the front routed {host} but nothing answers on {TURN_BACKEND} (is sew-media running?)"));
    }
    Err(format!("the front did not route {host}"))
}

/// S3, in Sew's order: the front serves → the TURN name is routed (proved
/// with a probe through the front) → `media reconfigure` to passthrough →
/// the TURN leaf is served through the front on loopback. Any step short
/// of that leaves (or puts) voice back on `direct`. A no-op when voice is
/// not installed or already on passthrough.
fn voice_to_passthrough() -> Result<Value, Refusal> {
    if !serving() {
        return Err((409, "front_not_serving", "the :443 front must be serving before TURN moves behind it".into()));
    }
    let env = crate::sew::supervisor::live();
    let st = crate::sew::voice::load(&env);
    if !st.installed || crate::sew::voice::tier(&st) == crate::sew::voice::TIER_PASSTHROUGH {
        return voice_tier(true);
    }
    if !env.ops.passthrough_supported() {
        return voice_tier(true); // the refusal, worded once (voice::set_tier)
    }
    let host = st.turn_host.clone().ok_or((409, "voice_not_installed", "no TURN host recorded".to_string()))?;
    set_turn_pending(Some(host.clone()));
    if let Err(e) = probe_route(&host) {
        set_turn_pending(None);
        return Err((409, "front_turn_route_unverified", format!("TURN stays on TCP 5349: {e}")));
    }
    log_debug!("[front443] TURN route proved through the front ({host} -> {TURN_BACKEND}); moving voice to passthrough");
    let moved = voice_tier(true);
    set_turn_pending(None);
    let mut v = moved?;
    // Verify: the TURN name through the front on loopback serves K2's leaf.
    let port = live_front().map(|(_, a)| a.port()).unwrap_or(443);
    let ours = st.cert_leaf_sha256.clone();
    let mut served = None;
    for _ in 0..40 {
        served = env.ops.served_leaf("127.0.0.1", &host, port);
        if served.is_some() && served == ours {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    if served.is_none() || served != ours {
        let back = voice_tier(false);
        rebuild_logged();
        return Err((
            500,
            "turn_passthrough_unverified",
            format!(
                "sew-media did not serve the TURN certificate through the front ({}); voice was moved back to TCP 5349{}",
                if served.is_some() { "another certificate" } else { "no TLS answer" },
                back.err().map(|(_, c, m)| format!(" — the move back failed too: {c}: {m}")).unwrap_or_default()
            ),
        ));
    }
    rebuild_logged();
    v["verified"] = json!({"through": format!("127.0.0.1:{port}"), "leafSha256": ours});
    Ok(v)
}

/// S5 (opt-in): the mail names → Stalwart's HTTPS on 127.0.0.1:8443, with
/// PROXY v2, loopback clients refused.
fn stalwart_routes(cfg: &k2_core::front443::FrontConfig, add: &mut Add) {
    if cfg.mail_behind_front() {
        for h in mail_names() {
            add(&h, routes::fixed_backend(Kind::Stalwart));
        }
    }
}

/// S4: Skin Direct's host (and the mail host while Caddy fronts mail) →
/// Caddy on its loopback port; PROXY v2 only when that Caddy reads it.
fn caddy_routes(add: &mut Add) {
    if let Some(c) = k2_core::skin_door::front_routes() {
        let addr = SocketAddr::from(([127, 0, 0, 1], c.port));
        for h in &c.hosts {
            add(h, Backend { kind: Kind::Caddy, addr, proxy: c.proxy_protocol, refuse_loopback_clients: false });
        }
    } else if !SKIN_SETTLED.load(Ordering::SeqCst) {
        // Boot: this daemon hasn't re-applied Skin Direct yet (Caddy
        // keeps running across the restart): keep its routes until it has.
        for (h, b) in routes::current().entries().filter(|(_, b)| b.kind == Kind::Caddy) {
            add(h, b.clone());
        }
    }
}

static SKIN_SETTLED: AtomicBool = AtomicBool::new(false);

/// Boot's Skin Direct apply finished (or there is none): Caddy routes now
/// come from the live Caddy only.
pub fn skin_settled() {
    SKIN_SETTLED.store(true, Ordering::SeqCst);
    rebuild_logged();
}

fn rebuild_logged() {
    let (t, w) = rebuild();
    for e in w {
        log_debug!("[front443] route not added: {e}");
    }
    let _ = t;
}

/// Stalwart's names behind the front: the mail host and the extra
/// mail-family names K2 issued certificates for.
fn mail_names() -> Vec<String> {
    if k2_core::db::try_shared().is_none() {
        return Vec::new();
    }
    let status = crate::mail::supervisor::row_field("status");
    if !matches!(status.as_deref(), Some("running" | "degraded" | "stopped")) {
        return Vec::new();
    }
    let mut v: Vec<String> = crate::mail::supervisor::row_field("hostname").into_iter().collect();
    v.extend(crate::mail::cert_names::load_state().into_keys());
    v
}

/// S3: move voice to `passthrough` (behind the serving front) or back to
/// `direct`.
fn voice_tier(passthrough: bool) -> Result<Value, Refusal> {
    let env = crate::sew::supervisor::live();
    crate::sew::voice::set_tier(&env, passthrough).map_err(|f| (500, "voice_tier_failed", format!("{}: {}", f.code, f.hint)))
}

/// S5: may Stalwart move behind the front?
fn mail_check() -> Result<(), String> {
    crate::mail::front_listener::check_can_move()
}

/// S5: move Stalwart's HTTPS listener behind the front (or back).
fn mail_move(on: bool) -> Result<Value, String> {
    crate::mail::front_listener::move_behind_front(on)
}

/// Voice follows the front: `passthrough` while the front serves (in Sew's
/// order, [`voice_to_passthrough`]), `direct` otherwise. Logged; never fatal.
fn converge_voice(passthrough: bool) {
    if passthrough && !(serving() && k2_core::front443::load().enabled) {
        return;
    }
    let r = if passthrough { voice_to_passthrough() } else { voice_tier(false) };
    match r {
        Ok(v) if v["changed"] == true => {
            log_debug!("[front443] voice moved to the {} TURN tier", v["turnTier"].as_str().unwrap_or("?"));
            rebuild_logged();
        }
        Ok(_) => {}
        Err((_, code, msg)) => log_debug!("[front443] voice tier not changed: {code}: {msg}"),
    }
}

/// `GET /cli/front/status`.
pub fn status_json() -> Value {
    let cfg = k2_core::front443::load();
    let (serving, source, local, counters, cap_now) = {
        let g = live().lock().unwrap_or_else(|p| p.into_inner());
        match g.as_ref() {
            Some(l) => (
                !l.accept.is_finished(),
                Some(l.source),
                l.local.map(|a| a.to_string()),
                l.front.counters.json(),
                l.front.cap,
            ),
            None => (false, None, None, Value::Null, cap()),
        }
    };
    let caddy = k2_core::skin_door::front_routes();
    let passthrough = match k2_core::front443::load_passthrough_checked() {
        Ok(v) => json!(v),
        Err(e) => json!({"error": e}),
    };
    json!({
        "enabled": cfg.enabled,
        "passthrough": passthrough,
        // Who holds :443 when the front doesn't (a web server that isn't
        // K2's is named, with the ways to fix it).
        "port443": if serving { Value::Null } else { port443_holder() },
        "guard": crate::apps::guard::counters_json(),
        "mail": cfg.mail_behind_front(),
        "serving": serving,
        "socket": {"source": source, "local": local, "systemd": systemd_fds::report()},
        "restartNeeded": cfg.enabled && !serving,
        "cap": cap_now,
        "e2eCap": crate::tunnel_tls_listener::max_e2e_connections(),
        "nofileSoftLimit": k2_core::nofile_soft_limit(),
        "connections": counters,
        "routes": routes::current().json(),
        "caddy": caddy.map(|c| json!({"port": c.port, "hosts": c.hosts, "proxyV2": c.proxy_protocol})),
        "helper": helper_gate().err(),
        // Enabled but not serving: systemd's own state and words for the
        // socket (why it didn't start), through the helper's open status.
        "socketUnit": if cfg.enabled && !serving && helper_gate().is_ok() {
            helper_front("status").map(|v| v["unit"].clone()).unwrap_or(Value::Null)
        } else {
            Value::Null
        },
    })
}

/// Why the root helper can't run the `front` verbs (`Ok` = it can).
fn helper_gate() -> Result<(), String> {
    let st = crate::root_helper::state();
    let ns = st.version_json.as_ref().and_then(|v| v.pointer("/namespaces/front")).and_then(Value::as_bool) == Some(true);
    if st.usable(FRONT_MIN_PROTOCOL) && ns {
        Ok(())
    } else {
        Err(crate::root_helper::helper_needed("the :443 front", FRONT_MIN_PROTOCOL, &st))
    }
}

/// A refusal: (HTTP status, code, message).
pub type Refusal = (u16, &'static str, String);

fn helper_front(verb: &str) -> Result<Value, String> {
    let out = crate::root_helper::call_text(&["front".to_string(), verb.to_string()], None)?;
    serde_json::from_str(&out).map_err(|e| format!("root helper answer: {e}"))
}

fn reapply_skin(daemon_port: u16) -> Option<String> {
    if !k2_core::skin_door::has_stored_front_door() {
        return None;
    }
    match k2_core::skin_door::apply(daemon_port) {
        Ok(_) => None,
        Err(e) => Some(format!("Skin Direct's Caddy: {e}")),
    }
}

fn gate() -> Result<(), Refusal> {
    if !cfg!(target_os = "linux") {
        return Err((409, "unsupported", "the :443 front runs on Linux boxes (a systemd socket)".into()));
    }
    if std::env::var_os("INVOCATION_ID").is_none() {
        return Err((409, "not_systemd", "this daemon is not running under systemd: the front needs its unit".into()));
    }
    let (_user, _unit, scope) = crate::root_helper::own_unit();
    if scope != "system" {
        return Err((409, "user_scope", "a user-scope daemon can't hold :443 (system units only)".into()));
    }
    helper_gate().map_err(|e| (409, "helper_needed", e))
}

/// `POST /cli/front/enable`. Blocking. `Ok` says whether the daemon must
/// restart to receive the socket (the route then restarts it).
pub fn enable(mail: bool, daemon_port: u16) -> Result<Value, Refusal> {
    gate()?;
    // A web server that isn't K2's holds :443: say so with the fixes, never
    // fight for the port (systemd's socket would fail to bind anyway).
    if !serving() {
        let h = port443_holder();
        if h["holder"] == "foreign" {
            return Err((409, "port_443_taken", h["message"].as_str().unwrap_or("another web server holds :443").to_string()));
        }
    }
    let prev = k2_core::front443::load();
    if mail && !prev.mail_behind_front() {
        mail_check().map_err(|e| (409, "mail_not_movable", e))?;
    }
    let now = chrono::Utc::now().timestamp();
    let cfg = k2_core::front443::FrontConfig { enabled: true, mail: mail || prev.mail_behind_front(), changed_at: Some(now) };
    k2_core::front443::save(&cfg).map_err(|e| (500, "config_write_failed", e))?;
    let mut warnings: Vec<String> = Vec::new();
    // Free :443: Caddy to loopback, Stalwart to 127.0.0.1:8443.
    if let Some(w) = reapply_skin(daemon_port) {
        warnings.push(w);
    }
    let undo = |warnings: &mut Vec<String>, moved_mail: bool| {
        if moved_mail {
            if let Err(e) = mail_move(false) {
                warnings.push(format!("Stalwart could not move back to :443: {e}"));
            }
        }
        let _ = k2_core::front443::save(&prev);
        if let Some(w) = reapply_skin(daemon_port) {
            warnings.push(w);
        }
    };
    let moved_mail = cfg.mail && !prev.mail_behind_front();
    if moved_mail {
        if let Err(e) = mail_move(true) {
            undo(&mut warnings, false);
            return Err((500, "mail_move_failed", format!("Stalwart did not move behind the front: {e}")));
        }
    }
    let helper = match helper_front("install") {
        Ok(v) => v,
        Err(e) => {
            undo(&mut warnings, moved_mail);
            return Err((409, "front_install_failed", format!("{e}{}", if warnings.is_empty() { String::new() } else { format!(" ({})", warnings.join("; ")) })));
        }
    };
    // systemd passes the socket only to a service it starts, and won't
    // start the socket while this service is active, so the daemon takes it
    // at its next start: the helper scheduled that one restart (systemctl
    // restart of this unit, a few seconds out). A hand-off can't carry it:
    // the successor isn't started by systemd.
    let needs_restart = !serving();
    let restart = needs_restart && helper["restartScheduled"] == true;
    if needs_restart && !restart {
        let (_user, unit, _scope) = crate::root_helper::own_unit();
        warnings.push(format!("the front is installed; restart the daemon to take port 443: `systemctl restart {unit}` (as root)"));
    }
    let (table, w) = rebuild();
    warnings.extend(w);
    // TURN moves only once the front serves (Sew's order): now, when it
    // already does; else at the restart's boot.
    let mut voice = Value::Null;
    if !restart {
        match voice_to_passthrough() {
            Ok(v) => voice = v,
            Err((_, code, msg)) => warnings.push(format!("voice: {code}: {msg}")),
        }
    }
    Ok(json!({
        "ok": true,
        "enabled": true,
        "mail": cfg.mail,
        "helper": helper,
        "voice": voice,
        "restarting": restart,
        "routes": table.json(),
        "warnings": warnings,
        "restartNeeded": needs_restart,
        "next": if restart {
            "K2 will restart its daemon once to take port 443 (a few seconds; agents resume). Voice moves to TURN on 443 when it is back."
        } else if needs_restart {
            "restart the daemon to take port 443"
        } else {
            "serving"
        },
    }))
}


/// `POST /cli/front/disable`. Blocking.
pub fn disable(daemon_port: u16) -> Result<Value, Refusal> {
    let cfg = k2_core::front443::load();
    let mut warnings: Vec<String> = Vec::new();
    // TURN back on public 5349 first: once the front is gone nothing
    // reaches sew-media's loopback listener.
    let voice = voice_tier(false)?;
    let stopped = stop();
    let helper = if cfg!(target_os = "linux") && helper_gate().is_ok() {
        match helper_front("remove") {
            Ok(v) => Some(v),
            Err(e) => return Err((500, "front_remove_failed", format!("the front stopped serving, but the socket stayed: {e}"))),
        }
    } else {
        None
    };
    if cfg.mail_behind_front() {
        if let Err(e) = mail_move(false) {
            warnings.push(format!("Stalwart is still on 127.0.0.1:8443: {e}"));
        }
    }
    let now = chrono::Utc::now().timestamp();
    k2_core::front443::save(&k2_core::front443::FrontConfig { enabled: false, mail: false, changed_at: Some(now) })
        .map_err(|e| (500, "config_write_failed", e))?;
    if let Some(w) = reapply_skin(daemon_port) {
        warnings.push(w);
    }
    routes::install(RouteTable::default());
    Ok(json!({
        "ok": true,
        "enabled": false,
        "stopped": stopped,
        "helper": helper,
        "voice": voice,
        "warnings": warnings,
    }))
}

// ── who holds :443 (0.46.6) ──────────────────────────────────────────

/// What holds :443 when the front doesn't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Holder443 {
    /// Nothing listens on :443.
    Free,
    /// The daemon's front serves it.
    Front,
    /// Skin Direct's Caddy (K2's own).
    SkinDirect,
    /// Stalwart's HTTPS listener (K2's hosted mail).
    Mail,
    /// A web server that isn't K2's: the active units K2 recognised (may be
    /// empty: something listens, K2 can't tell what).
    Foreign { units: Vec<String> },
    /// Not Linux / can't read the socket table.
    Unknown,
}

/// Units that commonly hold :443 (checked with `systemctl is-active`).
pub const KNOWN_WEB_UNITS: &[&str] = &["caddy", "nginx", "apache2", "httpd", "haproxy", "traefik", "lighttpd", "envoy", "openresty"];

/// Pure: decide from what K2 knows. A recognised web server unit that is
/// active wins over K2's own guesses (a foreign proxy can hold :443 on a
/// box that also runs hosted mail).
pub fn classify_443(
    listening: Option<bool>,
    front_serving: bool,
    skin_direct_443: bool,
    mail_on_443: bool,
    foreign_units: Vec<String>,
) -> Holder443 {
    if front_serving {
        return Holder443::Front;
    }
    match listening {
        None => Holder443::Unknown,
        Some(false) => Holder443::Free,
        Some(true) if !foreign_units.is_empty() => Holder443::Foreign { units: foreign_units },
        Some(true) if skin_direct_443 => Holder443::SkinDirect,
        Some(true) if mail_on_443 => Holder443::Mail,
        Some(true) => Holder443::Foreign { units: Vec::new() },
    }
}

/// The ways out when a web server that isn't K2's holds :443.
pub fn foreign_443_message(units: &[String]) -> String {
    let who = if units.is_empty() {
        "Another program (not K2) holds port 443 on this box".to_string()
    } else {
        format!("A web server that isn't K2's holds port 443 on this box ({})", units.join(", "))
    };
    format!(
        "{who}, so K2's :443 front can't take it and custom App names can't be served from this box's own IP yet. \
         Ways to fix it: (1) move that server to a loopback port, e.g. 127.0.0.1:9443 (it keeps its own sites and \
         certificates), then run k2 front enable and k2 front passthrough add <its host> --to 127.0.0.1:9443 (or '*' \
         for every name K2 doesn't serve): its sites keep working and K2 serves your App names; (2) keep it and have it \
         proxy the App's name to the App yourself, and add the name with k2 app domain add <app-id> <name> \
         --external so the App takes it as its origin; or (3) use K2 Connect for this name \
         (path b, coming in 0.46.7). K2 never takes :443 from another server."
    )
}

/// Is anything listening on TCP :443 (any address)? Linux only: the kernel
/// socket table (`/proc/net/tcp{,6}`, state LISTEN). `None` elsewhere.
fn listening_443() -> Option<bool> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let mut seen_any = false;
    for f in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(t) = std::fs::read_to_string(f) else { continue };
        seen_any = true;
        if proc_net_listens(&t, 443) {
            return Some(true);
        }
    }
    seen_any.then_some(false)
}

/// Pure: does a `/proc/net/tcp`-format table have a LISTEN socket on `port`?
pub fn proc_net_listens(table: &str, port: u16) -> bool {
    let want = format!("{port:04X}");
    table.lines().skip(1).any(|l| {
        let mut cols = l.split_whitespace();
        let (_sl, local, _rem, st) = (cols.next(), cols.next(), cols.next(), cols.next());
        let port_hex = local.and_then(|a| a.rsplit_once(':')).map(|(_, p)| p);
        port_hex.is_some_and(|p| p.eq_ignore_ascii_case(&want)) && st == Some("0A")
    })
}

fn active_web_units() -> Vec<String> {
    KNOWN_WEB_UNITS
        .iter()
        .filter(|u| {
            k2_core::bg_proc::command("systemctl")
                .args(["is-active", "--quiet", &format!("{u}.service")])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        })
        .map(|u| u.to_string())
        .collect()
}

/// `{holder, units?, message}` for status, setup and the doctor. Blocking
/// (reads /proc, may run systemctl).
pub fn port443_holder() -> Value {
    let front = serving();
    let listening = if front { Some(true) } else { listening_443() };
    let units = if listening == Some(true) && !front && cfg!(target_os = "linux") { active_web_units() } else { Vec::new() };
    let skin = k2_core::skin_door::live_listen().is_some_and(|l| l == ":443");
    let mail_on = !mail_names().is_empty() && !k2_core::front443::mail_behind_front();
    match classify_443(listening, front, skin, mail_on, units) {
        Holder443::Front => json!({"holder": "k2-front", "message": "the :443 front serves it"}),
        Holder443::Free => json!({"holder": "free", "message": "nothing listens on :443"}),
        Holder443::SkinDirect => json!({
            "holder": "k2-skin-direct",
            "message": "Skin Direct's Caddy (K2's) holds :443; k2 front enable moves it behind the front"
        }),
        Holder443::Mail => json!({
            "holder": "k2-mail",
            "message": "hosted mail's HTTPS (K2's Stalwart) holds :443; k2 front enable --mail moves it behind the front"
        }),
        Holder443::Unknown => json!({
            "holder": "unknown",
            "message": "K2 can't read this box's socket table (the :443 front is Linux-only)"
        }),
        Holder443::Foreign { units } => json!({"holder": "foreign", "units": units, "message": foreign_443_message(&units)}),
    }
}

// ── owner passthrough (0.46.6) ───────────────────────────────────────

/// `GET /cli/front/passthrough`.
pub fn passthrough_json() -> Result<Value, Refusal> {
    let list = k2_core::front443::load_passthrough_checked().map_err(|e| (500, "passthrough_unreadable", e))?;
    Ok(json!({
        "ok": true,
        "passthrough": list,
        "frontEnabled": k2_core::front443::load().enabled,
        "routes": routes::current().json(),
    }))
}

fn passthrough_host(raw: &str) -> Result<String, Refusal> {
    let t = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    if t == k2_core::front443::PASSTHROUGH_DEFAULT {
        return Ok(t);
    }
    client_hello::host_name(t.as_bytes()).ok_or_else(|| {
        (400, "bad_host", format!("not a host name: {raw:?} (a name like shop.example.com, or '*' for every name K2 doesn't serve)"))
    })
}

/// `POST /cli/front/passthrough/add {host, to, proxyV2?, label?}`. Owner/Admin.
pub fn passthrough_add(host: &str, to: &str, proxy_v2: bool, label: &str, by: &str, daemon_port: u16) -> Result<Value, Refusal> {
    let host = passthrough_host(host)?;
    // `k2 publish` visibility (vs-live PA-4): a nested name stays behind the
    // site gate, whatever its mode. `*` itself is fine: it skips these names.
    if k2_core::tunnel::subdomains::current().is_nested_name(&host) {
        return Err((409, "host_is_k2s", format!("{host} can't be passed through: it is one of this server's published addresses, and K2 serves those itself. Use a name you own.")));
    }
    let addr: SocketAddr = to.trim().parse().map_err(|_| {
        (400, "bad_upstream", format!("--to must be a loopback address and port, e.g. 127.0.0.1:9443 (got {to:?})"))
    })?;
    if !addr.ip().is_loopback() {
        return Err((
            400,
            "upstream_not_loopback",
            format!("{addr} is not on this box's loopback: move the server to 127.0.0.1:<port> first"),
        ));
    }
    if addr.port() == 0 || addr.port() == daemon_port || k2_core::front443::RESERVED_BACKEND_PORTS.contains(&addr.port()) {
        return Err((409, "upstream_port_reserved", format!("port {} is K2's own; pick another loopback port (e.g. 9443)", addr.port())));
    }
    if host != k2_core::front443::PASSTHROUGH_DEFAULT {
        if let Some(why) = current_use(&host).filter(|w| !w.contains("is passed through")) {
            return Err((409, "host_is_k2s", format!("{why}: K2 serves that name itself")));
        }
        if crate::apps::tls_door::reload().routed.iter().any(|h| h.eq_ignore_ascii_case(&host)) {
            return Err((409, "host_is_k2s", format!("{host} is a custom App name K2 serves (k2 app domain rm first)")));
        }
    }
    let label: String = label.chars().filter(|c| !c.is_control()).take(80).collect();
    let mut list = k2_core::front443::load_passthrough_checked().map_err(|e| (500, "passthrough_unreadable", e))?;
    list.retain(|p| p.host != host);
    if list.len() >= k2_core::front443::MAX_PASSTHROUGH {
        return Err((409, "too_many", format!("at most {} passthrough names", k2_core::front443::MAX_PASSTHROUGH)));
    }
    list.push(k2_core::front443::Passthrough {
        host: host.clone(),
        to: addr,
        proxy_v2,
        label,
        added_at: chrono::Utc::now().timestamp(),
        added_by: by.to_string(),
    });
    list.sort_by(|a, b| a.host.cmp(&b.host));
    k2_core::front443::save_passthrough(&list).map_err(|e| (500, "config_write_failed", e))?;
    let (table, warnings) = rebuild_if_enabled();
    let enabled = k2_core::front443::load().enabled;
    Ok(json!({
        "ok": true,
        "host": host,
        "to": addr.to_string(),
        "proxyV2": proxy_v2,
        "passthrough": list,
        "routes": table,
        "warnings": warnings,
        "next": if enabled { "routed now" } else { "the :443 front is off: these route once the owner runs k2 front enable" },
    }))
}

/// `POST /cli/front/passthrough/rm {host}`. Owner/Admin.
pub fn passthrough_rm(host: &str) -> Result<Value, Refusal> {
    let host = passthrough_host(host)?;
    let mut list = k2_core::front443::load_passthrough_checked().map_err(|e| (500, "passthrough_unreadable", e))?;
    let before = list.len();
    list.retain(|p| p.host != host);
    if list.len() == before {
        return Err((404, "not_found", format!("{host} is not a passthrough name (k2 front passthrough)")));
    }
    k2_core::front443::save_passthrough(&list).map_err(|e| (500, "config_write_failed", e))?;
    let (table, warnings) = rebuild_if_enabled();
    Ok(json!({"ok": true, "removed": host, "passthrough": list, "routes": table, "warnings": warnings}))
}

fn rebuild_if_enabled() -> (Value, Vec<String>) {
    if !k2_core::front443::load().enabled {
        return (routes::current().json(), Vec::new());
    }
    let (t, w) = rebuild();
    (t.json(), w)
}
