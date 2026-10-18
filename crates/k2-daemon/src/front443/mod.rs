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
//! Owner surface: `GET /cli/front/status`, `POST /cli/front/enable`
//! (`{"mail": bool}`), `POST /cli/front/disable`; CLI `k2 front …`.

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
    let local = l.local_addr().ok();
    // A hand-off successor routes from its first connection with the
    // table its predecessor passed (fixed backends only). A cold start
    // (systemd's fd) accepts only once its own table is built — the DB is
    // open a few seconds in — and the kernel queues connections meanwhile
    // (backlog 4096), so nothing that arrives during boot is closed for a
    // route that simply isn't loaded yet.
    let handed = routes::take_handed();
    let (ready_tx, mut ready_rx) = tokio::sync::watch::channel(handed.is_some());
    if let Some(t) = handed {
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
    log_debug!("[front443] holding {local:?} (socket from {source}; cap {})", cap());
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
    }
    routes::install(t);
    (routes::current(), warnings)
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
    json!({
        "enabled": cfg.enabled,
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
