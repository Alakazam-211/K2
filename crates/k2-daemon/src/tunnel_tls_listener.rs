//! K2 Connect E2E HTTPS listener (PRD `k2-connect-e2e-encryption.md` §4
//! Option A — daemon-terminated TLS).
//!
//! A SECOND loopback listener, bound on its own ephemeral port, that
//! terminates TLS itself (rustls / aws-lc-rs) for the user's
//! `<sub>.k2.dev` subdomain. frpc forwards the *encrypted* SNI-routed
//! stream here (`type = "https"`), so the relay only ever carries
//! ciphertext. The TLS handshake completes here; the decrypted plaintext
//! is then handed to the SAME route dispatcher the cleartext HTTP listener
//! uses — there are **no dispatcher changes**.
//!
//! ## Why bridge rather than call `dispatch()` directly
//!
//! [`crate::routes::dispatcher::dispatch`] is hard-typed to
//! `tokio::net::TcpStream` (its keep-alive loop, the WS-upgrade handoff in
//! `events`/`sessions_*_ws`, etc. all take `&mut TcpStream`). Making it
//! generic over `AsyncRead + AsyncWrite` would ripple through every WS
//! handler — exactly the kind of churn the PRD says to avoid ("the
//! dispatcher is protocol-agnostic — no dispatcher changes"). Instead we
//! terminate TLS and **splice** the decrypted byte stream to the daemon's
//! own cleartext HTTP listener on `127.0.0.1:<http_port>`. That loopback
//! hop never leaves the machine, the bytes are already decrypted, and the
//! cleartext listener runs the unmodified `dispatch()` (keep-alive, WS
//! upgrades, everything). The TLS boundary is the daemon process; the
//! relay still sees only ciphertext.
//!
//! ## Feature gate
//!
//! The whole listener is gated on [`k2_core::tunnel::e2e_enabled`]; when
//! E2E is OFF (the default), [`maybe_spawn`] is a no-op and the daemon
//! behaves EXACTLY as today.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use arc_swap::ArcSwap;
use rustls::ServerConfig;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio_rustls::TlsAcceptor;

use k2_core::log_debug;

/// The live E2E listener: its port, the subdomain its certificate names,
/// and the tasks that serve it (accept loop + cert renewer).
///
/// **Not** a `OnceLock<u16>`: the accept task can die (panic, OS close, FD
/// exhaustion) while the daemon process stays alive. A OnceLock would keep
/// returning the **dead** port forever → frpc dials connection-refused →
/// silent external outage (luzz 2026-07-29, nsi/acv class).
///
/// **Keyed by subdomain** (0.45.2): the listener serves ONE name. Before
/// 0.45.2 a tunnel start reused any live listener, so after "release `afs`,
/// connect `k2n1-m4`" the daemon kept serving the `afs.k2.dev` certificate
/// until a restart (Reggie's Mac mini: `CertificateUnknown` for every
/// direct client). [`ensure_spawned_blocking`] now replaces a listener that
/// serves another name, and [`retire_listener`] is the identity-changed
/// hook. Serialized by [`SPAWN_LOCK`] on spawn.
struct LiveListener {
    /// Bumped on every spawn, so a dying accept task only clears the
    /// record it owns (a port number can be reused by the next bind).
    generation: u64,
    port: u16,
    /// Trimmed subdomain label exactly as the cert was requested for.
    subdomain: String,
    accept: tokio::task::JoinHandle<()>,
    renewer: Option<tokio::task::JoinHandle<()>>,
}

static LIVE: OnceLock<Mutex<Option<LiveListener>>> = OnceLock::new();
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Serializes concurrent on-demand spawns so two simultaneous tunnel starts
/// can't race into two listeners. Cheap — held only across the bind, not the
/// accept loop.
static SPAWN_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn spawn_lock() -> &'static Mutex<()> {
    SPAWN_LOCK.get_or_init(|| Mutex::new(()))
}

fn live_slot() -> &'static Mutex<Option<LiveListener>> {
    LIVE.get_or_init(|| Mutex::new(None))
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// `(port, subdomain)` of the live listener record, if any.
pub fn live_listener() -> Option<(u16, String)> {
    live_slot()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(|l| (l.port, l.subdomain.clone()))
}

/// The subdomain the running cert renewer works for (`None` when no
/// renewer runs: no listener, or the self-signed dev hatch).
pub fn live_renewer_subdomain() -> Option<String> {
    live_slot()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .filter(|l| l.renewer.as_ref().is_some_and(|r| !r.is_finished()))
        .map(|l| l.subdomain.clone())
}

/// Clear the record + on-disk publish when the dying accept task still
/// owns it (generation match), so ensure can re-bind.
fn clear_if_generation(generation: u64) {
    let mut slot = live_slot().lock().unwrap_or_else(|p| p.into_inner());
    if slot.as_ref().is_some_and(|l| l.generation == generation) {
        if let Some(l) = slot.take() {
            if let Some(r) = l.renewer {
                r.abort();
            }
        }
        k2_core::tunnel::tls::clear_https_port();
    }
}

/// A listener taken out of service by [`retire_listener`]: its tasks are
/// already aborted; tests may await `accept` to see the socket closed.
pub struct Retired {
    pub subdomain: String,
    pub port: u16,
    pub accept: tokio::task::JoinHandle<()>,
}

/// Tunnel identity changed: stop the listener and its cert renewer unless
/// they already serve `desired` (`None` = no listener wanted). Aborting the
/// accept task drops the `TcpListener` (the port closes); connections
/// already spliced finish on their own. Clears `tunnel-https.port`.
pub fn retire_listener(desired: Option<&str>) -> Option<Retired> {
    let mut slot = live_slot().lock().unwrap_or_else(|p| p.into_inner());
    if let (Some(l), Some(want)) = (slot.as_ref(), desired) {
        if same_name(&l.subdomain, want) {
            return None;
        }
    }
    let l = slot.take()?;
    l.accept.abort();
    if let Some(r) = l.renewer {
        r.abort();
    }
    k2_core::tunnel::tls::clear_https_port();
    log_debug!(
        "[daemon/e2e] tunnel identity changed — stopped the E2E listener on 127.0.0.1:{} \
         and the cert renewer for {}.k2.dev (now: {})",
        l.port,
        l.subdomain,
        desired.map(|d| format!("{d}.k2.dev")).unwrap_or_else(|| "no listener".into())
    );
    Some(Retired {
        subdomain: l.subdomain,
        port: l.port,
        accept: l.accept,
    })
}

/// Take the record out whatever it serves (dead accept loop, or another
/// name), aborting its tasks. Used under [`spawn_lock`] before a re-bind.
fn retire_any() {
    let mut slot = live_slot().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(l) = slot.take() {
        l.accept.abort();
        if let Some(r) = l.renewer {
            r.abort();
        }
        k2_core::tunnel::tls::clear_https_port();
    }
}

/// Loopback TCP probe — true if *something* accepts on `127.0.0.1:port`.
/// Used to detect a stale recorded port after the accept loop died.
fn loopback_port_live(port: u16) -> bool {
    if port == 0 {
        return false;
    }
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

/// How often the cert-renewal loop wakes to check the installed cert's
/// `notAfter`. 12 h is far finer than the ~30-day renewal window, so a cert
/// is always re-issued well before expiry even across a daemon restart, yet
/// the loop is near-idle (two cheap cert parses a day). Mirrors the
/// lease-renewal loop's "schedule lives in the daemon" pattern.
const CERT_CHECK_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);

// ─────────────────────────────────────────────────────────────────────
// H2 (E2E splice DoS hardening) — timeouts + a global connection cap
// ─────────────────────────────────────────────────────────────────────

/// Deadline for the inbound TLS handshake. A client (or the relay
/// forwarding ciphertext) that opens the TCP/TLS connection but never
/// completes the handshake would otherwise pin a spawned task — and a
/// semaphore permit — forever (a classic slowloris). 10s is generous for
/// a real handshake over any sane network yet bounds the half-open hold.
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Deadline for dialing the local splice backend (the daemon's own
/// cleartext HTTP listener, or a configured nested target). Connecting to
/// loopback is ~instant; a target that's slow/unreachable must not hang
/// the per-connection task. Short by design — this is a same-host dial.
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Floor and ceiling of the process-global cap on concurrently-served E2E
/// TLS connections. Each accepted connection takes one permit BEFORE its
/// task is spawned and holds it for the connection's life (handshake +
/// splice), so the listener can't be driven to FD exhaustion / unbounded
/// task growth by a flood of connections.
///
/// K-M13 (0.46.1): the cap was a fixed 256. Published services, skin
/// gateways and Apps all ride this listener now, so it is sized from
/// `RLIMIT_NOFILE` (raised to the hard limit at boot):
/// [`k2_core::connection_cap`] — an eighth of the soft limit, never below
/// the old 256, at most 16384.
const E2E_CAP_FLOOR: usize = 256;
const E2E_CAP_CEILING: usize = 16_384;

/// The E2E connection cap for this process (fixed at first use, which is
/// after boot raised the soft limit).
pub fn max_e2e_connections() -> usize {
    static CAP: OnceLock<usize> = OnceLock::new();
    *CAP.get_or_init(|| k2_core::connection_cap(E2E_CAP_FLOOR, E2E_CAP_CEILING))
}

/// The global E2E connection semaphore (lazily created, lives for the
/// process). Shared by every accept loop (there is normally exactly one).
fn conn_semaphore() -> &'static Arc<Semaphore> {
    static SEM: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEM.get_or_init(|| Arc::new(Semaphore::new(max_e2e_connections())))
}

/// Spawn the E2E HTTPS listener when the `K2_E2E` flag is on. Returns the
/// chosen HTTPS port (for the tunnel connector / logging) or `None` when
/// E2E is disabled. Errors are returned (caller logs + continues — a failed
/// E2E listener must never take the daemon down).
///
/// `http_port` is the daemon's existing cleartext HTTP listener port — the
/// decrypted stream is spliced there so the unmodified dispatcher serves it.
pub async fn maybe_spawn(http_port: u16) -> Result<Option<u16>, String> {
    if k2_core::airgap::enabled() {
        log_debug!(
            "[daemon/e2e] air-gap is on (K2_AIRGAP=1) — skipping maybe_spawn (no cert.k2.dev)"
        );
        return Ok(None);
    }
    let cfg = match k2_core::tunnel::config::load() {
        Ok(c) => c,
        Err(e) => return Err(format!("load tunnel config: {e}")),
    };
    if !k2_core::tunnel::e2e_enabled(&cfg) {
        return Ok(None); // user opted out (e2e:false / K2_E2E falsey) — legacy path.
    }

    // A subdomain is required to name the cert SANs. At boot a brand-new user
    // may have E2E on (now the default) but no subdomain configured yet — that
    // is NOT an error condition, just "nothing to stand up yet": the listener
    // is brought up on demand by `ensure_spawned` once they configure a
    // subdomain and start the tunnel. Return None (no listener) rather than a
    // loud error so the common first-boot case stays quiet.
    let subdomain = cfg.subdomain.trim().to_string();
    if subdomain.is_empty() {
        log_debug!(
            "[daemon/e2e] E2E on but no subdomain configured yet — deferring the \
             HTTPS listener until the tunnel is started with a subdomain"
        );
        return Ok(None);
    }

    // Through the same serialized, liveness-aware ensure the tunnel start
    // uses (0.46.2), on the blocking pool: with no usable cert the broker
    // call can take minutes (`CERT_ISSUE_TIMEOUT`), which must neither pin
    // a runtime worker nor race a concurrent tunnel start into a second
    // listener.
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || ensure_spawned_blocking(handle, http_port))
        .await
        .map_err(|e| format!("E2E listener bring-up task failed: {e}"))?
        .map(Some)
}

/// Bind + spawn the E2E HTTPS listener for `subdomain`, provisioning its cert
/// via the broker seam first, and record it in [`LIVE`] (port, name, tasks).
/// Idempotent guard lives in the callers; this is the actual bring-up.
async fn spawn_listener(http_port: u16, subdomain: String) -> Result<u16, String> {
    spawn_listener_with(http_port, subdomain, RenewSchedule::production()).await
}

/// [`spawn_listener`] with an explicit renewer schedule (tests).
async fn spawn_listener_with(
    http_port: u16,
    subdomain: String,
    schedule: RenewSchedule,
) -> Result<u16, String> {
    let subdomain = subdomain.trim().to_string();
    // Cert + key via the broker seam (broker-issued on the real path;
    // self-signed only under the explicit `K2_E2E_SELF_SIGNED` spike hatch).
    // An installed cert for this name that has NOT expired is served as-is,
    // also inside the renewal window (0.46.2): the renewer spawned below
    // renews it in the background. Only when there is no usable cert
    // (missing, expired, another name) does the broker have to answer
    // here; a failure is then returned — the caller logs it and the tunnel
    // does NOT come up against an untrusted/missing cert (no silent
    // cleartext fallback). A cert on disk for ANOTHER name is never reused
    // (`cert_covers`).
    let (cert_pem, key_pem) = k2_core::tunnel::tls::load_or_provision_cert(&subdomain)
        .map_err(|e| format!("provision tunnel cert: {e}"))?;
    let server_config = k2_core::tunnel::tls::server_config(&cert_pem, &key_pem)
        .map_err(|e| format!("build rustls ServerConfig: {e}"))?;

    // Hold the live ServerConfig behind an ArcSwap so the renewal loop can
    // HOT-SWAP a freshly-issued chain in WITHOUT dropping the listener: each
    // new connection reads the current config, so a swap takes effect on the
    // next handshake with zero downtime.
    let shared_config: Arc<ArcSwap<ServerConfig>> =
        Arc::new(ArcSwap::from_pointee(server_config));

    // The loopback listener (127.0.0.1 only — frpc dials it locally).
    // Runtime stage H (AR3/K37): the port is STABLE. A successor daemon
    // takes the predecessor's listening socket over (hand-off); otherwise
    // the port recorded in `tunnel-https.sticky-port` is re-bound when
    // free, so frpc's `localPort` (and an adopted frpc) stays valid across
    // re-binds and restarts. Only a port held by something else moves.
    let listener = Arc::new(take_or_claim_listener()?);
    crate::handoff::register(crate::handoff::Role::TunnelTls, &listener);
    let https_port = listener
        .local_addr()
        .map_err(|e| format!("read HTTPS listener addr: {e}"))?
        .port();

    k2_core::tunnel::tls::publish_https_port(https_port)
        .map_err(|e| format!("publish HTTPS port: {e}"))?;

    log_debug!(
        "[daemon/e2e] K2 Connect E2E TLS listener up on 127.0.0.1:{} \
         (subdomain {}.k2.dev; decrypted → HTTP :{})",
        https_port,
        subdomain,
        http_port
    );

    let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let accept = {
        let cfg = shared_config.clone();
        tokio::spawn(async move {
            // If the accept task ends or panics, clear the record so the
            // next ensure/self-heal rebinds instead of returning a dead
            // port (luzz silent outage class). Only this generation's
            // record: a newer listener may own the slot already.
            struct ClearOnDrop {
                generation: u64,
                port: u16,
            }
            impl Drop for ClearOnDrop {
                fn drop(&mut self) {
                    log_debug!(
                        "[daemon/e2e] E2E accept loop on port {} ended (generation {})",
                        self.port,
                        self.generation
                    );
                    clear_if_generation(self.generation);
                }
            }
            let _guard = ClearOnDrop { generation, port: https_port };
            accept_loop(listener, cfg, http_port).await;
        })
    };

    // Auto-renewal (PRD §6): re-issue ~30d pre-expiry and hot-swap the
    // config. Skipped under the self-signed spike hatch (a self-signed cert
    // has no broker to renew against; the spike is a local dev convenience).
    let renewer = if !k2_core::tunnel::cert_broker::self_signed_mode() {
        let cfg = shared_config.clone();
        let sub = subdomain.clone();
        Some(tokio::spawn(async move {
            cert_renewal_loop(sub, cfg, schedule).await;
        }))
    } else {
        None
    };

    let previous = live_slot()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .replace(LiveListener {
            generation,
            port: https_port,
            subdomain,
            accept,
            renewer,
        });
    // Callers retire first; a leftover here would serve a second listener.
    if let Some(old) = previous {
        old.accept.abort();
        if let Some(r) = old.renewer {
            r.abort();
        }
    }

    Ok(https_port)
}

/// The E2E listener socket: the one a hand-off passed in (once), else a
/// fresh bind on the sticky port.
fn take_or_claim_listener() -> Result<TcpListener, String> {
    let std_listener = match crate::handoff::take_inherited(crate::handoff::Role::TunnelTls) {
        Some(l) => {
            log_debug!("[daemon/e2e] using the E2E listener handed over by the previous daemon");
            l
        }
        None => {
            let (l, port, reused) = k2_core::tunnel::tls::claim_sticky_loopback(
                &k2_core::tunnel::tls::https_sticky_port_path(),
            )
            .map_err(|e| format!("bind E2E HTTPS listener: {e}"))?;
            if !reused {
                log_debug!(
                    "[daemon/e2e] E2E listener bound a NEW port {port} (sticky port missing or \
                     taken) — frpc localPort follows it"
                );
            }
            l
        }
    };
    std_listener
        .set_nonblocking(true)
        .map_err(|e| format!("E2E listener set_nonblocking: {e}"))?;
    TcpListener::from_std(std_listener).map_err(|e| format!("adopt E2E listener: {e}"))
}

/// On-demand bring-up of the E2E HTTPS listener, callable from a BLOCKING
/// context (the tunnel connector's `spawn_blocking` worker). This is the
/// robust first-connect path now that E2E is the default: a tunnel start
/// ENSURES the listener exists — provisioning the cert + binding if needed —
/// before frpc forwards to it, instead of erroring when boot never published
/// a port (e.g. the subdomain was configured after the daemon booted).
///
/// Idempotent + serialized + **liveness- and name-aware**:
///   * if a listener is recorded, still accepts on loopback, **and serves
///     the subdomain now in `tunnel.json`**, return it;
///   * if recorded but **dead** (accept task gone — luzz 2026-07-29) or
///     serving **another name** (subdomain switch, 0.45.2), stop it and
///     re-bind for the configured name (new ephemeral port, cert for the
///     new name);
///   * otherwise takes [`spawn_lock`], re-checks (double-checked locking),
///     and drives the async [`spawn_listener`] to completion on the provided
///     tokio runtime `handle` (we're on a blocking thread, so `block_on`).
///
/// Errors loud — issuance/bind failure or a still-empty subdomain returns an
/// `Err` the connector propagates; it never silently forwards cleartext.
fn ensure_spawned_blocking(handle: tokio::runtime::Handle, http_port: u16) -> Result<u16, String> {
    // Runtime stage H: a daemon that handed off never binds again.
    k2_core::handoff_state::guard()?;
    if k2_core::airgap::enabled() {
        return Err(k2_core::airgap::TEACHING.to_string());
    }
    let cfg = k2_core::tunnel::config::load().map_err(|e| format!("load tunnel config: {e}"))?;
    // Defensive: only the E2E path should reach here, but never bind a TLS
    // listener for an opted-out config.
    if !k2_core::tunnel::e2e_enabled(&cfg) {
        return Err(
            "ensure_https_listener called but E2E is disabled (opted out) — \
             this is a bug; the connector must only ensure the listener on the \
             E2E path"
                .to_string(),
        );
    }
    let subdomain = cfg.subdomain.trim().to_string();
    if subdomain.is_empty() {
        return Err(
            "cannot stand up the E2E HTTPS listener: no subdomain configured in \
             ~/.k2/tunnel.json — select/claim a subdomain in Settings → K2 Connect \
             first (the cert SANs need <sub>.k2.dev)"
                .to_string(),
        );
    }

    let serves_config = |live: Option<(u16, String)>| -> Option<u16> {
        match live {
            Some((p, sub)) if same_name(&sub, &subdomain) && loopback_port_live(p) => Some(p),
            _ => None,
        }
    };
    // Fast path: the live listener serves the configured name.
    if let Some(p) = serves_config(live_listener()) {
        return Ok(p);
    }

    let _g = spawn_lock().lock().unwrap_or_else(|p| p.into_inner());
    // Re-check under the lock — another ensure may have respawned while we
    // waited.
    let live = live_listener();
    if let Some(p) = serves_config(live.clone()) {
        return Ok(p);
    }
    if let Some((p, sub)) = live {
        if same_name(&sub, &subdomain) {
            log_debug!(
                "[daemon/e2e] recorded HTTPS port {p} is not accepting — will re-bind \
                 (listener death / silent external outage class)"
            );
        } else {
            log_debug!(
                "[daemon/e2e] E2E listener on :{p} serves {sub}.k2.dev but the tunnel is \
                 now {subdomain}.k2.dev — replacing it (subdomain switch)"
            );
        }
        retire_any();
    }

    // Drive the async bind/provision/spawn to completion from this blocking
    // thread. `block_on` on a borrowed Handle is valid from a non-runtime
    // thread (the connector calls us inside `spawn_blocking`).
    handle.block_on(spawn_listener(http_port, subdomain))
}

/// Install the k2-core seams so the (k2-core) tunnel connector and the
/// tunnel-identity path can demand / retire this (k2-daemon) listener
/// without a layering inversion. Call once at boot, AFTER a tokio runtime
/// exists — we capture its [`Handle`] so the blocking hook can drive the
/// async bring-up. See [`k2_core::tunnel::tls::register_ensure_https_listener`]
/// and [`k2_core::tunnel::tls::register_retire_https_listener`].
pub fn register_ensure_hook() {
    let handle = tokio::runtime::Handle::current();
    k2_core::tunnel::tls::register_ensure_https_listener(Box::new(move |http_port| {
        ensure_spawned_blocking(handle.clone(), http_port)
    }));
    k2_core::tunnel::tls::register_retire_https_listener(Box::new(|desired| {
        retire_listener(desired).map(|r| r.subdomain)
    }));
}

/// Accept TLS connections forever, handing each to a per-connection task.
/// Reads the CURRENT [`ServerConfig`] from `shared_config` on every accept
/// so a renewal swap is picked up by the next connection with no listener
/// restart.
async fn accept_loop(
    listener: Arc<TcpListener>,
    shared_config: Arc<ArcSwap<ServerConfig>>,
    http_port: u16,
) {
    loop {
        let accepted = tokio::select! {
            r = listener.accept() => r,
            // Runtime stage H: the successor takes the socket over. Stop
            // accepting without closing it (the hand-off holds a ref).
            _ = crate::handoff::quiesced() => {
                log_debug!("[daemon/e2e] accept loop stopped for the hand-off");
                return;
            }
        };
        match accepted {
            Ok((tcp, peer)) => {
                // H2: take a global connection permit BEFORE spawning the
                // per-connection task so the listener can't be driven to FD
                // exhaustion / unbounded task growth. We `try_acquire` (never
                // block the accept loop): at the cap we DROP this connection
                // immediately — keeping the open-FD count bounded — rather
                // than queueing it (queued half-open conns would still hold
                // FDs). The permit is held for the connection's whole life
                // (moved into the task) and auto-released on task end.
                let permit = match Arc::clone(conn_semaphore()).try_acquire_owned() {
                    Ok(p) => p,
                    Err(_) => {
                        log_debug!(
                            "[daemon/e2e] connection cap ({}) reached \
                             — dropping inbound connection from {peer}",
                            max_e2e_connections()
                        );
                        drop(tcp); // close the FD now; do not spawn.
                        continue;
                    }
                };
                // Snapshot the live config for THIS connection (cheap Arc
                // clone). A concurrent hot-swap only affects later accepts.
                let acceptor = TlsAcceptor::from(shared_config.load_full());
                // Runtime stage H: the connection lives on the SPLICE
                // runtime, not the main one, so it outlives a hand-off (the
                // old daemon shuts its main runtime down and drains these).
                // Moved across runtimes through a std socket (a tokio
                // socket is bound to the reactor that registered it).
                let std_tcp = match tcp.into_std() {
                    Ok(s) => s,
                    Err(e) => {
                        log_debug!("[daemon/e2e] detach accepted socket from {peer}: {e}");
                        continue;
                    }
                };
                crate::handoff::splice_handle().spawn(async move {
                    // Hold the permit for the life of the connection; it is
                    // released when this task (and thus `_permit`) drops.
                    let _permit = permit;
                    let _splice = crate::handoff::splice_guard();
                    let tcp = match TcpStream::from_std(std_tcp) {
                        Ok(t) => t,
                        Err(e) => {
                            log_debug!("[daemon/e2e] adopt socket from {peer}: {e}");
                            return;
                        }
                    };
                    if let Err(e) = serve_one(tcp, acceptor, http_port).await {
                        // 0.40.68 / GH#57: include peer so client-side flaps
                        // (repeated handshake eof from one source) are
                        // diagnosable without packet capture. Peer is the
                        // frpc local hop (usually 127.0.0.1) when E2E sits
                        // behind the tunnel — still distinguishes concurrent
                        // dialers and timestamps the storm.
                        log_debug!("[daemon/e2e] connection ended peer={peer}: {e}");
                    }
                });
            }
            Err(e) => {
                // Transient accept errors: log + continue. Fatal/closed
                // listener: exit the loop so ClearPortOnDrop runs and
                // ensure can re-bind (without this we spin forever with a
                // dead recorded port).
                let fatal = e.kind() == std::io::ErrorKind::ConnectionAborted
                    || e.kind() == std::io::ErrorKind::NotConnected
                    || e.kind() == std::io::ErrorKind::BrokenPipe
                    || e.raw_os_error() == Some(9) // EBADF
                    || e.raw_os_error() == Some(22) // EINVAL (closed on some platforms)
                    || e.to_string().contains("os error 9");
                if fatal {
                    log_debug!(
                        "[daemon/e2e] fatal accept error — ending accept loop so \
                         ensure can re-bind: {e}"
                    );
                    break;
                }
                log_debug!("[daemon/e2e] accept error: {e}");
            }
        }
    }
}

/// When the cert renewer runs: how often it checks a fresh cert, and how
/// long it waits after a failed renewal (index = failures so far − 1; the
/// last entry repeats). Injected so tests run the real loop in milliseconds.
#[derive(Debug, Clone)]
pub(crate) struct RenewSchedule {
    pub check_every: Duration,
    pub retry_after_failure: Vec<Duration>,
}

impl RenewSchedule {
    /// Production: check every 12 h; after a failed renewal retry in 1 m,
    /// 2 m, 5 m, 15 m, 30 m, then hourly. A cert inside the 30-day window
    /// still serves meanwhile, so hourly retries leave hundreds of tries.
    pub(crate) fn production() -> Self {
        Self {
            check_every: CERT_CHECK_INTERVAL,
            retry_after_failure: [60, 120, 300, 900, 1800, 3600]
                .into_iter()
                .map(Duration::from_secs)
                .collect(),
        }
    }

    /// The wait after `failures` consecutive failed renewals (≥ 1).
    pub(crate) fn retry_delay(&self, failures: u32) -> Duration {
        crate::tunnel_autostart::backoff_step(&self.retry_after_failure, failures)
    }
}

/// Daemon-owned cert-renewal loop (mirrors `tunnel::lease`'s daemon-side
/// scheduling). Checks the installed leaf's `notAfter` AT ONCE and then
/// every `schedule.check_every`; when it's within
/// [`k2_core::tunnel::cert_broker::RENEWAL_WINDOW`] of expiry, re-runs the
/// broker client and hot-swaps the rustls config in place.
///
/// Fully fault-isolated: a failed renewal is logged and retried on the
/// backoff in `schedule.retry_after_failure` (the OLD cert keeps serving
/// until the new one lands), so a hiccup never drops the listener. The
/// first check runs immediately (0.46.2): the listener now starts on a
/// still-valid cert inside the window instead of renewing inline, and this
/// loop is what renews it. Runs until the listener is retired (its task is
/// aborted) or the tunnel serves another name.
async fn cert_renewal_loop(
    subdomain: String,
    shared_config: Arc<ArcSwap<ServerConfig>>,
    schedule: RenewSchedule,
) {
    use k2_core::tunnel::cert_broker;

    log_debug!(
        "[daemon/e2e] cert auto-renewal started for {}.k2.dev (check now, then every {:?}; \
         renew within {:?} of expiry)",
        subdomain,
        schedule.check_every,
        cert_broker::RENEWAL_WINDOW
    );

    let mut failures: u32 = 0;
    loop {
        // Belt to the identity-changed abort (0.45.2): a renewer for a name
        // the tunnel no longer serves must never request that name's cert
        // (with the NEW bearer) and overwrite `tunnel-cert.pem`.
        let configured = k2_core::tunnel::config::load()
            .map(|c| c.subdomain)
            .unwrap_or_default();
        if !same_name(&configured, &subdomain) {
            log_debug!(
                "[daemon/e2e] cert renewer for {subdomain}.k2.dev stopping: the tunnel now \
                 serves {configured:?}"
            );
            return;
        }

        // Read the installed cert to decide whether we're inside the window.
        // Unreadable counts as "needs renewal" (the broker reinstalls it).
        let cert_path = k2_core::tunnel::tls::cert_path();
        let installed = match std::fs::read_to_string(&cert_path) {
            Ok(s) => s,
            Err(e) => {
                log_debug!("[daemon/e2e] renewal: cannot read {}: {e}", cert_path.display());
                String::new()
            }
        };
        if !cert_broker::cert_needs_renewal(
            &installed,
            cert_broker::RENEWAL_WINDOW,
            cert_broker::now_unix(),
        ) {
            failures = 0;
            tokio::time::sleep(schedule.check_every).await;
            continue; // still fresh — quiet on the happy path.
        }

        log_debug!(
            "[daemon/e2e] cert for {}.k2.dev is within the renewal window — \
             requesting a fresh chain from the broker",
            subdomain
        );

        // Re-provision via the broker. Blocking reqwest under the hood, so
        // run it off the async runtime.
        let sub = subdomain.clone();
        let provisioned =
            tokio::task::spawn_blocking(move || cert_broker::provision_via_broker(&sub)).await;
        let outcome = match provisioned {
            Ok(Ok((cert_pem, key_pem))) => k2_core::tunnel::tls::server_config(&cert_pem, &key_pem)
                .map_err(|e| format!("fresh cert didn't build a ServerConfig: {e}")),
            Ok(Err(e)) => Err(format!("broker request failed: {e}")),
            Err(e) => Err(format!("provision task panicked: {e}")),
        };
        match outcome {
            Ok(new_cfg) => {
                shared_config.store(Arc::new(new_cfg));
                failures = 0;
                log_debug!(
                    "[daemon/e2e] cert for {}.k2.dev renewed + hot-swapped (no listener restart)",
                    subdomain
                );
                tokio::time::sleep(schedule.check_every).await;
            }
            Err(e) => {
                failures = failures.saturating_add(1);
                let wait = schedule.retry_delay(failures);
                let state = if cert_broker::cert_expired(&installed, cert_broker::now_unix()) {
                    "the EXPIRED cert"
                } else {
                    "the installed cert, still valid"
                };
                log_debug!(
                    "[daemon/e2e] WARN: cert renewal for {}.k2.dev failed (attempt {failures}; \
                     serving {state}; retrying in {:?}): {e}",
                    subdomain,
                    wait
                );
                tokio::time::sleep(wait).await;
            }
        }
    }
}

/// Terminate TLS on `tcp`, then route the decrypted stream by the
/// connection's **SNI host** (PRD §7 Pro multi-subdomain):
///
///   * the **primary** `<sub>.k2.dev` (or any host we don't recognise as a
///     *configured* nested label) → splice to the daemon's own cleartext
///     HTTP listener on `127.0.0.1:<http_port>` (the EXISTING behaviour —
///     the unmodified `dispatch()` serves it);
///   * a **configured nested** label `<label>.<sub>.k2.dev` → proxy the
///     decrypted stream to its internal `target` endpoint (e.g.
///     `localhost:3000`, dialed as 127.0.0.1 then ::1); if no address
///     accepts, the visitor gets a `502 Bad Gateway`, never a 404;
///   * an **unknown / unprovisioned nested** label → a clean 404 + close
///     (never leak an unprovisioned nested host to the daemon).
///
/// The routing decision is a single O(1) map lookup
/// ([`k2_core::tunnel::subdomains::current`]) on the SNI the handshake
/// already produced — no extra read, no accept-loop blocking. With no nested
/// subdomains configured (the default / pre-refresh state) the map is empty
/// and every host routes to the daemon — byte-for-byte today's behaviour.
// H3 (WS-aware front-side idle timeout) — DELIBERATELY SKIPPED here.
//
// The requirement is an idle timeout that RESETS on bytes in EITHER
// direction so it bounds a post-handshake slowloris dribble without
// killing a healthy long-lived WebSocket. `tokio::io::copy_bidirectional`
// does not surface per-direction byte activity, so honouring that "reset
// on any byte" contract would mean replacing it with a hand-rolled copy
// loop that arms/disarms a timer around every read on both halves. Getting
// that wrong tears down a live WS (its server-push frames or the client's
// pings can be quieter than any timeout we pick), which the PRD explicitly
// forbids ("SKIP if it risks killing live WS; don't break WS"). The two
// real DoS vectors H3 targets are ALREADY bounded elsewhere: the half-open
// handshake by `TLS_HANDSHAKE_TIMEOUT` above, and the post-handshake
// "open a conn and go silent" case by the dispatcher's existing
// `KEEP_ALIVE_IDLE_TIMEOUT` on the spliced Daemon backend. The connection
// cap caps the blast radius regardless. So we leave the live splice on
// `copy_bidirectional` rather than risk a WS regression. Revisit only with
// a WS-frame-aware idle accounting (a real proxy), not a blunt byte timer.

/// Dial a same-host splice backend with the H2 connect timeout applied.
/// Used for the daemon path (`(host, port)` tuple). Nested targets go
/// through [`crate::upstream_dial::dial_target`] instead (localhost rule +
/// per-address error log).
async fn connect_upstream<A: tokio::net::ToSocketAddrs>(addr: &A) -> Result<TcpStream, String> {
    match tokio::time::timeout(UPSTREAM_CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
        Ok(Ok(s)) => Ok(s),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!(
            "upstream connect timed out after {UPSTREAM_CONNECT_TIMEOUT:?}"
        )),
    }
}

/// Body of the 502 a visitor gets when a nested label's app is not
/// accepting connections. Deliberately says nothing about the internal
/// target (the address and errors go to the daemon log only).
pub(crate) const UNREACHABLE_BODY: &str =
    "Service not reachable: the app behind this address is not accepting connections.\n";

/// Best-effort: read the visitor's request head (so closing the socket does
/// not reset it before the reply lands), then answer `502 Bad Gateway` and
/// close the TLS stream cleanly.
async fn write_unreachable_502<S>(tls: &mut S)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut head = Vec::new();
    let mut buf = [0u8; 4096];
    while head.len() < 64 * 1024 && !head.windows(4).any(|w| w == b"\r\n\r\n") {
        match tokio::time::timeout(Duration::from_secs(2), tls.read(&mut buf)).await {
            Ok(Ok(n)) if n > 0 => head.extend_from_slice(&buf[..n]),
            _ => break,
        }
    }
    let resp = format!(
        "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain; charset=utf-8\r\n\
         Cache-Control: no-store\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n{}",
        UNREACHABLE_BODY.len(),
        k2_core::frame_policy::CONNECT_HEADER_LINES,
        UNREACHABLE_BODY
    );
    let _ = tls.write_all(resp.as_bytes()).await;
    let _ = tls.flush().await;
    let _ = tls.shutdown().await;
}

async fn serve_one(
    mut tcp: TcpStream,
    acceptor: TlsAcceptor,
    http_port: u16,
) -> Result<(), String> {
    use k2_core::tunnel::subdomains::{self, Route};
    use tokio::io::AsyncWriteExt;

    // H2: bound the TLS handshake. A peer that opens the connection but
    // never finishes the handshake (slowloris) would otherwise pin this
    // task — and its connection permit — indefinitely. The timeout frees
    // both deterministically. One deadline covers frpc's PROXY header and
    // the handshake.
    let deadline = tokio::time::Instant::now() + TLS_HANDSHAKE_TIMEOUT;

    // 0.46.3: frpc (`transport.proxyProtocolVersion = "v2"`) puts a PROXY
    // v2 header carrying the address the relay saw in front of the TLS
    // bytes. This listener is frpc's `localPort` on loopback and the only
    // one that reads such a header (trust rules: `crate::client_ip`).
    // `None` = no header, an old relay, or not a loopback peer: Apps then
    // get no client-IP headers at all, never a client's.
    let peer = tcp.peer_addr().map_err(|e| format!("peer address: {e}"))?;
    let client_ip = crate::client_ip::read_tunnel_preamble(&mut tcp, peer, deadline)
        .await
        .map_err(|e| format!("TLS handshake failed: {e}"))?;

    let mut tls = match tokio::time::timeout_at(deadline, acceptor.accept(tcp)).await {
        Ok(Ok(tls)) => tls,
        Ok(Err(e)) => return Err(format!("TLS handshake failed: {e}")),
        Err(_) => {
            return Err(format!(
                "TLS handshake timed out after {TLS_HANDSHAKE_TIMEOUT:?} (slowloris/half-open) — dropped"
            ))
        }
    };

    // The SNI the client sent (set at the TLS layer, before any HTTP bytes —
    // exactly what the relay routes `*.<sub>.k2.dev` on). `None` for a
    // no-SNI client → treated as an empty host → routes to the daemon.
    let sni = tls
        .get_ref()
        .1
        .server_name()
        .map(|s| s.to_string())
        .unwrap_or_default();

    match subdomains::current().route_for_host(&sni) {
        Route::Daemon => {
            // PRD connect-login-edge-only I2: splice to the TUNNEL-INGRESS
            // listener (dispatch tagged `Ingress::Tunnel`), NOT the
            // privileged main HTTP listener (`http_port`, `Ingress::Loopback`).
            // The ingress listener is re-bound on demand if it died; a
            // failure here drops the connection LOUDLY — never a silent
            // fallback to `http_port` (that would reopen the public login
            // surface the PRD closes). `http_port` stays for diagnostics.
            let ingress_port = crate::tunnel_ingress_listener::ensure_port()
                .await
                .map_err(|e| {
                    format!(
                        "tunnel-ingress listener unavailable (refusing to splice to \
                         main HTTP :{http_port}): {e}"
                    )
                })?;
            // The dispatcher's keep-alive loop owns request framing.
            // H2: bound the upstream dial (loopback is ~instant; a wedged
            // backend must not hang this task).
            let mut upstream = connect_upstream(&("127.0.0.1", ingress_port))
                .await
                .map_err(|e| format!("connect to tunnel-ingress listener :{ingress_port}: {e}"))?;
            // The dispatcher reads the client IP back by this hop's address
            // (login throttle, auth audit). Registered before any byte.
            let _hop = crate::client_ip::register_hop(&upstream, client_ip);
            let res = tokio::io::copy_bidirectional(&mut tls, &mut upstream)
                .await
                .map(|_| ())
                .map_err(|e| format!("splice TLS↔HTTP: {e}"));
            // H6: emit close_notify on a clean splice end so a strict client
            // sees an orderly TLS close rather than an unexpected EOF.
            let _ = tls.shutdown().await;
            res
        }
        Route::Internal(target) => {
            // App runtime (AR4): a deploy moves an App's backend by the
            // daemon-local override, never by a control-plane write. Only a
            // label the control plane already routes here can be overridden.
            let label = subdomains::current().nested_label(&sni);
            if let Some((label, be)) =
                label.and_then(|l| crate::apps::router::live(&l).map(|be| (l, be)))
            {
                log_debug!("[daemon/e2e] routing {sni} → App backend :{}", be.port);
                let mode = crate::apps::router::rewrite_mode(&be, &label, crate::client_ip::Mode::Tunnel(client_ip));
                if let Err(e) = crate::apps::router::splice(&mut tls, be, mode).await {
                    log_debug!("[daemon/e2e] {sni}: {e} — answering 502");
                    write_unreachable_502(&mut tls).await;
                    return Ok(());
                }
                let _ = tls.shutdown().await;
                return Ok(());
            }
            // Configured nested subdomain → proxy the decrypted stream to the
            // user's chosen internal endpoint on this same machine.
            // H2: bound the upstream dial. `localhost` targets try 127.0.0.1
            // then ::1 (never the resolver alone) — an IPv4-only app on a box
            // whose `localhost` resolves to `::1` was unreachable before.
            let mut upstream =
                match crate::upstream_dial::dial_target(&target, UPSTREAM_CONNECT_TIMEOUT).await {
                    Ok(s) => s,
                    Err(fail) => {
                        // Answer the visitor with a 502, not a dropped
                        // connection (which the edge surfaced as a 404).
                        log_debug!(
                            "[daemon/e2e] {sni}: upstream {fail} — answering 502"
                        );
                        write_unreachable_502(&mut tls).await;
                        return Ok(());
                    }
                };
            // 0.46.3: a target on this daemon's own main port (any alias)
            // would make internet traffic look local. Refuse it before a
            // byte is forwarded, and mark every other splice so the main
            // listener still serves it as tunnel ingress if it lands there.
            if let Err(why) = crate::splice_guard::refuse_self_dial(&upstream) {
                log_debug!("[daemon/e2e] {sni}: refusing target {target}: {why} — answering 502");
                drop(upstream);
                write_unreachable_502(&mut tls).await;
                return Ok(());
            }
            let _splice_mark = crate::splice_guard::mark(&upstream);
            log_debug!("[daemon/e2e] routing {sni} → internal endpoint {target}");
            // A nested service stays a raw splice (any protocol), as before
            // 0.46.3, unless its label opted in to the client-IP rewrite
            // (`k2 publish client-ip <label> on`). The hop lets an App front
            // port on the other end keep the address either way.
            let _hop = crate::client_ip::register_hop(&upstream, client_ip);
            let opted_in = subdomains::current()
                .nested_label(&sni)
                .is_some_and(|label| k2_core::tunnel::client_ip_labels::enabled(&label));
            let res = if opted_in {
                let mode = crate::client_ip::Mode::Tunnel(client_ip);
                crate::client_ip::forward(&mut tls, &mut upstream, mode)
                    .await
                    .map_err(|e| format!("splice TLS↔internal {target}: {e}"))
            } else {
                tokio::io::copy_bidirectional(&mut tls, &mut upstream)
                    .await
                    .map(|_| ())
                    .map_err(|e| format!("splice TLS↔internal {target}: {e}"))
            };
            // H6: clean close_notify after the splice on the nested path too.
            let _ = tls.shutdown().await;
            res
        }
        Route::UnknownNested => {
            // A nested label with no configured target. Reply 404 + close —
            // do NOT silently fall through to the daemon (an unprovisioned
            // nested host is not a daemon route).
            log_debug!("[daemon/e2e] rejecting unconfigured nested subdomain {sni} (404)");
            let body = "Unknown subdomain";
            let resp = format!(
                "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\n\
                 Content-Length: {}\r\n{frame}Connection: close\r\n\r\n{body}",
                body.len(),
                frame = k2_core::frame_policy::CONNECT_HEADER_LINES,
            );
            let _ = tls.write_all(resp.as_bytes()).await;
            let _ = tls.flush().await;
            // Clean TLS shutdown (sends close_notify) so a strict client sees
            // an orderly close rather than an unexpected-EOF after the 404.
            let _ = tls.shutdown().await;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// With the user OPTED OUT (`K2_E2E=0`, the env opt-out), `maybe_spawn`
    /// is a no-op and returns `None` — the legacy terminating path, no HTTPS
    /// listener. We force the opt-out via env so the test is deterministic
    /// regardless of any real `~/.k2/tunnel.json` on the box (E2E is ON by
    /// default as of 0.40.6+, so "no env" no longer means off).
    #[tokio::test]
    async fn maybe_spawn_is_noop_when_user_opted_out() {
        // Shared env lock held for the whole test; restored on drop.
        let _e2e = k2_core::test_env::EnvVar::set("K2_E2E", "0"); // explicit opt-out wins → OFF
        let res = maybe_spawn(12345)
            .await
            .expect("opt-out must not error");
        assert!(res.is_none(), "opt-out must yield no HTTPS listener");
    }

    #[tokio::test]
    async fn maybe_spawn_skips_cert_when_airgap_even_with_leftover_subdomain() {
        let _home = crate::test_support::TempHome::new();
        let _e2e = k2_core::test_env::EnvVar::remove("K2_E2E"); // default ON — leftover subdomain would POST
        let _air = k2_core::test_env::EnvVar::set("K2_AIRGAP", "1");

        let spy = std::net::TcpListener::bind("127.0.0.1:0").expect("spy bind");
        spy.set_nonblocking(true).expect("nonblocking");
        let spy_port = spy.local_addr().expect("addr").port();
        let _broker = k2_core::test_env::EnvVar::set(
            k2_core::tunnel::cert_broker::BROKER_URL_ENV,
            format!("http://127.0.0.1:{spy_port}/cert"),
        );

        let mut cfg = k2_core::tunnel::config::TunnelConfig::default();
        cfg.subdomain = "leftover".into();
        cfg.token = "tok".into();
        k2_core::tunnel::config::save(&cfg).expect("save leftover tunnel.json");

        let res = maybe_spawn(12345)
            .await
            .expect("airgap must not error");
        assert!(res.is_none(), "airgap must skip maybe_spawn entirely");
        assert!(
            spy.accept().is_err(),
            "leftover subdomain must not HTTP to cert.k2.dev when air-gap is on"
        );
    }

    #[tokio::test]
    async fn maybe_spawn_fresh_tunnel_json_still_skips_cert_when_airgap_off() {
        let _home = crate::test_support::TempHome::new();
        let _air = k2_core::test_env::EnvVar::remove("K2_AIRGAP");
        let spy = std::net::TcpListener::bind("127.0.0.1:0").expect("spy bind");
        spy.set_nonblocking(true).expect("nonblocking");
        let spy_port = spy.local_addr().expect("addr").port();
        let _broker = k2_core::test_env::EnvVar::set(
            k2_core::tunnel::cert_broker::BROKER_URL_ENV,
            format!("http://127.0.0.1:{spy_port}/cert"),
        );
        // Fresh / empty subdomain: no cert POST even with air-gap off.
        k2_core::tunnel::config::save(&k2_core::tunnel::config::TunnelConfig::default())
            .expect("save empty tunnel.json");
        let res = maybe_spawn(12345)
            .await
            .expect("empty subdomain must not error");
        assert!(res.is_none(), "fresh tunnel.json must not spawn E2E");
        assert!(
            spy.accept().is_err(),
            "fresh tunnel.json must not POST cert.k2.dev at boot"
        );
    }

    /// End-to-end: with a self-signed cert, the HTTPS listener accepts a
    /// real TLS connection and routes a request through the dispatcher
    /// (proven by a live `/ping` 200 over TLS). Exercises the full path —
    /// rustls handshake → decrypt → splice → dispatch — without the env
    /// flag (we call the internals directly so the test is hermetic).
    #[tokio::test]
    async fn https_listener_routes_a_request_through_the_dispatcher() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

        // Sandbox $HOME FIRST (the env lock): it serializes this test with
        // every other user of the process-wide tunnel-ingress listener, and
        // the self-signed leaf is minted fresh for "rosson" into a throwaway
        // `.k2/` rather than reusing a real cert in this box's `~/.k2`.
        let _home = crate::test_support::TempHome::new();

        // 1) Install the daemon state the TUNNEL-INGRESS listener dispatches
        //    with (PRD connect-login-edge-only I2: the TLS splice now lands on
        //    the real dispatcher tagged `Ingress::Tunnel`, brought up on
        //    demand by `ensure_port`). `http_port` is the main listener's
        //    port — deliberately a port NOTHING listens on, so the test
        //    proves the splice does not go there.
        let http_port = install_test_ingress_state();
        // `/account` sits behind the readiness gate (503 until ready).
        crate::boot_status::set_ready();

        // 2) Provision a self-signed cert + build the rustls acceptor. This
        //    test exercises the TLS→splice path, not the broker, so engage
        //    the explicit dev/spike self-signed escape hatch
        //    (`K2_E2E_SELF_SIGNED=1`) — the default path is broker-issued.
        let _self_signed = k2_core::test_env::EnvVar::set("K2_E2E_SELF_SIGNED", "1");
        let (cert_pem, key_pem) = k2_core::tunnel::tls::load_or_provision_cert("rosson")
            .expect("provision self-signed cert");
        let server_config =
            k2_core::tunnel::tls::server_config(&cert_pem, &key_pem).expect("server config");
        // Hold it behind the same ArcSwap the production listener uses so the
        // test exercises the real accept path (per-connection config read).
        let shared = Arc::new(ArcSwap::from_pointee(server_config));

        // 3) Bind the HTTPS listener + run the accept loop.
        let tls_listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind tls listener");
        let https_port = tls_listener.local_addr().unwrap().port();
        {
            let shared = shared.clone();
            tokio::spawn(async move {
                accept_loop(Arc::new(tls_listener), shared, http_port).await;
            });
        }

        // 4) Connect a rustls CLIENT that trusts our self-signed cert and
        //    send GET /ping; assert we get the 200/pong back through the
        //    whole TLS→splice→stub-dispatcher path.
        let body = tls_client_get(&cert_pem, https_port, "rosson.k2.dev", "/ping").await;
        assert!(
            body.contains("200 OK") && body.contains(crate::BANNER),
            "expected the real dispatcher's /ping banner over TLS via the tunnel-ingress \
             listener, got:\n{body}"
        );
        // I2 proof: the splice landed on the TUNNEL-INGRESS dispatcher, so
        // the account page is not served (G1): with a configured tunnel
        // label it is a 302 to the hosted web client — a splice to the main
        // listener would have served the HTML.
        k2_core::tunnel::config::save(&k2_core::tunnel::config::TunnelConfig {
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            ..Default::default()
        })
        .expect("seed tunnel config");
        let page = tls_client_get(&cert_pem, https_port, "rosson.k2.dev", "/account").await;
        assert!(
            page.starts_with("HTTP/1.1 302 Found\r\n")
                && page.contains("\r\nLocation: https://rosson.app.k2.dev/\r\n")
                && !page.contains("<html"),
            "tunnel ingress must 302 the account page to the web client, got:\n{page}"
        );
    }

    /// Minimal rustls client that trusts the given self-signed leaf cert
    /// (added to a fresh root store), connects to 127.0.0.1:`port` with the
    /// SNI `sni`, sends `GET <path>`, and returns the raw response text.
    async fn tls_client_get(cert_pem: &str, port: u16, sni: &str, path: &str) -> String {
        use rustls_pemfile::Item;
        use tokio_rustls::TlsConnector;

        let mut roots = rustls::RootCertStore::empty();
        for item in rustls_pemfile::read_all(&mut cert_pem.as_bytes()) {
            if let Ok(Item::X509Certificate(der)) = item {
                roots.add(der).expect("add self-signed cert to roots");
            }
        }
        let client_config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(client_config));

        let tcp = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("client connect");
        let server_name = rustls::pki_types::ServerName::try_from(sni.to_string())
            .expect("valid SNI");
        let mut tls = connector
            .connect(server_name, tcp)
            .await
            .expect("client TLS handshake");

        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: {sni}\r\nConnection: close\r\n\r\n"
        );
        tls.write_all(req.as_bytes()).await.expect("write req");
        tls.flush().await.expect("flush");

        let mut buf = Vec::new();
        tls.read_to_end(&mut buf).await.expect("read resp");
        String::from_utf8_lossy(&buf).into_owned()
    }

    /// Stand up a tiny cleartext HTTP stub that answers EVERY request with a
    /// 200 carrying `tag` as the body. Stands in for either the daemon's own
    /// HTTP listener or a user's internal endpoint. Returns its bound port.
    async fn spawn_tagged_stub(tag: &'static str) -> u16 {
        let l = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind stub");
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = match l.accept().await {
                    Ok(v) => v,
                    Err(_) => return,
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = s.read(&mut buf).await;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        tag.len(),
                        tag
                    );
                    let _ = s.write_all(resp.as_bytes()).await;
                    let _ = s.flush().await;
                });
            }
        });
        port
    }

    /// PRD connect-login-edge-only I2: `Route::Daemon` splices to the
    /// tunnel-ingress listener (real dispatcher, `Ingress::Tunnel`), so the
    /// TLS tests install a throwaway `DaemonState` for it and assert against
    /// the real `/ping` banner. Returns a "main HTTP port" that NOTHING
    /// listens on — proof the splice never falls back to it.
    /// Install the test daemon state AND forget any listener an earlier
    /// test's (now dropped) runtime owned. Callers hold a `TempHome` (the
    /// env lock) first, which serializes every user of this global.
    fn install_test_ingress_state() -> u16 {
        crate::tunnel_ingress_listener::forget_recorded_port_for_test();
        let (event_tx, _rx) = tokio::sync::broadcast::channel::<crate::events::WireEvent>(
            crate::events::EVENT_CHANNEL_CAP,
        );
        crate::tunnel_ingress_listener::install_state(crate::DaemonState {
            token: Arc::new("tls-test-owner-token".to_string()),
            started_at: std::time::Instant::now(),
            port: 1,
            event_tx: Arc::new(event_tx),
            shutdown_tx: None,
            login_limiter: Arc::new(crate::login_throttle::LoginLimiter::new()),
        });
        1
    }

    /// SNI Host routing (PRD §7): the primary `<sub>.k2.dev` reaches the
    /// daemon (via the tunnel-ingress listener); a CONFIGURED nested label reaches its internal endpoint;
    /// an UNKNOWN nested label is rejected with a 404 (never the daemon).
    /// Exercises the real `serve_one` path (TLS terminate → SNI lookup →
    /// route) via the production accept loop + the global subdomain cache.
    #[tokio::test]
    async fn sni_routes_primary_to_daemon_nested_to_endpoint_unknown_404() {
        use k2_core::tunnel::subdomains::{self, SubdomainMap};

        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

        // Acquire the crate-wide serialization lock FIRST (sandboxes $HOME and
        // serializes against every other global-state test). This must wrap the
        // whole body because we mutate the process-global subdomain routing map
        // below — a concurrent tunnel test resetting that map would otherwise
        // misroute our nested label.
        let _home = crate::test_support::TempHome::new();

        // Two distinct backends so we can prove WHERE a connection landed:
        // the real dispatcher (tunnel-ingress) vs a tagged internal stub.
        let daemon_port = install_test_ingress_state();
        let internal_port = spawn_tagged_stub("INTERNAL").await;

        // Seed the global routing map: primary `rosson`, nested
        // `staging → internal endpoint`. (`ghost` is intentionally absent.)
        let mut targets = std::collections::HashMap::new();
        targets.insert("staging".to_string(), format!("127.0.0.1:{internal_port}"));
        subdomains::store(SubdomainMap {
            primary: "rosson".to_string(),
            targets,
        });

        // Self-signed cert covering rosson.k2.dev + *.rosson.k2.dev (the
        // per-user wildcard), so every SNI below presents a trusted name.
        let _self_signed = k2_core::test_env::EnvVar::set("K2_E2E_SELF_SIGNED", "1");
        let (cert_pem, key_pem) =
            k2_core::tunnel::tls::load_or_provision_cert("rosson").expect("cert");
        let server_config =
            k2_core::tunnel::tls::server_config(&cert_pem, &key_pem).expect("server config");
        let shared = Arc::new(ArcSwap::from_pointee(server_config));

        // The HTTPS listener splices Daemon-routed traffic to `daemon_port`.
        let tls_listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind tls");
        let https_port = tls_listener.local_addr().unwrap().port();
        {
            let shared = shared.clone();
            tokio::spawn(async move {
                accept_loop(Arc::new(tls_listener), shared, daemon_port).await;
            });
        }

        // Primary → the daemon (real dispatcher via tunnel-ingress).
        let primary = tls_client_get(&cert_pem, https_port, "rosson.k2.dev", "/ping").await;
        assert!(
            primary.contains("200 OK") && primary.contains(crate::BANNER),
            "primary host must reach the daemon, got:\n{primary}"
        );

        // Configured nested → internal endpoint.
        let nested = tls_client_get(&cert_pem, https_port, "staging.rosson.k2.dev", "/x").await;
        assert!(
            nested.contains("200 OK") && nested.contains("INTERNAL"),
            "configured nested label must reach its internal endpoint, got:\n{nested}"
        );

        // Unknown nested → 404 from the listener itself (NOT the daemon).
        let unknown = tls_client_get(&cert_pem, https_port, "ghost.rosson.k2.dev", "/ping").await;
        assert!(
            unknown.contains("404 Not Found") && !unknown.contains(crate::BANNER),
            "unknown nested label must be rejected 404, not routed to the daemon, got:\n{unknown}"
        );

        // Reset the global cache so we don't bleed into sibling tests.
        subdomains::store(SubdomainMap::default());
    }

    /// The field bug: a nested label targets `localhost:PORT`, the app
    /// listens on 127.0.0.1 only, and the box resolves `localhost` to `::1`.
    /// The proxy must reach the app whatever the resolver says (the dial
    /// never asks it for `localhost`). And a label whose app is down gets a
    /// 502, not a 404 and not a silent close.
    #[tokio::test]
    async fn nested_localhost_target_reaches_ipv4_app_and_dead_app_is_502() {
        use k2_core::tunnel::subdomains::{self, SubdomainMap};

        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let _home = crate::test_support::TempHome::new();
        let daemon_port = install_test_ingress_state();

        // IPv4-only app (spawn_tagged_stub binds 127.0.0.1).
        let app_port = spawn_tagged_stub("V4APP").await;
        // A port nothing listens on, on either loopback (reserved for the
        // test's life so a parallel test can't take it).
        let closed = k2_core::test_env::ClosedPort::dual();
        let dead_port = closed.port();

        let mut targets = std::collections::HashMap::new();
        targets.insert("web".to_string(), format!("localhost:{app_port}"));
        targets.insert("down".to_string(), format!("localhost:{dead_port}"));
        subdomains::store(SubdomainMap {
            primary: "example".to_string(),
            targets,
        });

        let _self_signed = k2_core::test_env::EnvVar::set("K2_E2E_SELF_SIGNED", "1");
        let (cert_pem, key_pem) =
            k2_core::tunnel::tls::load_or_provision_cert("example").expect("cert");
        let server_config =
            k2_core::tunnel::tls::server_config(&cert_pem, &key_pem).expect("server config");
        let shared = Arc::new(ArcSwap::from_pointee(server_config));
        let tls_listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind tls");
        let https_port = tls_listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            accept_loop(Arc::new(tls_listener), shared, daemon_port).await;
        });

        let ok = tls_client_get(&cert_pem, https_port, "web.example.k2.dev", "/").await;
        assert!(
            ok.starts_with("HTTP/1.1 200 OK\r\n") && ok.contains("V4APP"),
            "localhost target must reach the IPv4-only app, got:\n{ok}"
        );

        let down = tls_client_get(&cert_pem, https_port, "down.example.k2.dev", "/").await;
        assert!(
            down.starts_with("HTTP/1.1 502 Bad Gateway\r\n")
                && down.contains(UNREACHABLE_BODY)
                && !down.contains("404"),
            "unreachable app must answer 502 (not 404, not a silent close), got:\n{down}"
        );
        assert!(
            !down.contains(&dead_port.to_string()),
            "the 502 body must not leak the internal target, got:\n{down}"
        );

        subdomains::store(SubdomainMap::default());
    }

    /// 0.46.3: a nested label whose target is the daemon's own main port
    /// is answered 502 under every loopback alias, and not one request byte
    /// reaches that port. A normal app target on the same box still works.
    #[tokio::test]
    async fn nested_target_on_daemon_port_is_502_under_every_alias() {
        use k2_core::tunnel::subdomains::{self, SubdomainMap};
        use std::sync::atomic::{AtomicUsize, Ordering};

        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let _home = crate::test_support::TempHome::new();
        let daemon_port = install_test_ingress_state();

        // Stand-in for the daemon's main listener: dual-stack, so the v4
        // aliases arrive as `::ffff:127.0.0.1`. Counts accepted sockets and
        // sockets that delivered any byte.
        let main = TcpListener::bind("[::]:0")
            .await
            .expect("bind [::]:0 (this test needs dual-stack loopback)");
        let main_port = main.local_addr().unwrap().port();
        let accepted = Arc::new(AtomicUsize::new(0));
        let got_bytes = Arc::new(AtomicUsize::new(0));
        {
            let (accepted, got_bytes) = (accepted.clone(), got_bytes.clone());
            tokio::spawn(async move {
                while let Ok((mut s, _)) = main.accept().await {
                    accepted.fetch_add(1, Ordering::SeqCst);
                    let got_bytes = got_bytes.clone();
                    tokio::spawn(async move {
                        let mut b = [0u8; 256];
                        if matches!(s.read(&mut b).await, Ok(n) if n > 0) {
                            got_bytes.fetch_add(1, Ordering::SeqCst);
                            let _ = s
                                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nMAIN")
                                .await;
                        }
                    });
                }
            });
        }
        let _reg = crate::splice_guard::register_daemon_port(main_port);
        let app_port = spawn_tagged_stub("APP").await;

        let aliases = [
            ("v4", format!("127.0.0.1:{main_port}"), true),
            ("lh", format!("localhost:{main_port}"), true),
            ("v6", format!("[::1]:{main_port}"), true),
            ("any", format!("0.0.0.0:{main_port}"), false),
        ];
        let mut targets = std::collections::HashMap::new();
        for (label, t, _) in &aliases {
            targets.insert(label.to_string(), t.clone());
        }
        targets.insert("app".to_string(), format!("localhost:{app_port}"));
        subdomains::store(SubdomainMap { primary: "example".to_string(), targets });

        let _self_signed = k2_core::test_env::EnvVar::set("K2_E2E_SELF_SIGNED", "1");
        let (cert_pem, key_pem) =
            k2_core::tunnel::tls::load_or_provision_cert("example").expect("cert");
        let server_config =
            k2_core::tunnel::tls::server_config(&cert_pem, &key_pem).expect("server config");
        let shared = Arc::new(ArcSwap::from_pointee(server_config));
        let tls_listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind tls");
        let https_port = tls_listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            accept_loop(Arc::new(tls_listener), shared, daemon_port).await;
        });

        for (label, target, must_connect) in &aliases {
            let before = accepted.load(Ordering::SeqCst);
            let sni = format!("{label}.example.k2.dev");
            let resp = tls_client_get(&cert_pem, https_port, &sni, "/cli/auth/login").await;
            assert!(
                resp.starts_with("HTTP/1.1 502 Bad Gateway\r\n") && !resp.contains("MAIN"),
                "{target}: a target on the daemon port must be 502, got:\n{resp}"
            );
            if *must_connect {
                assert!(
                    accepted.load(Ordering::SeqCst) > before,
                    "{target}: the dial reached the daemon port (the belt, not a refused dial, answered)"
                );
            }
        }
        // Give any (wrongly) forwarded bytes time to land.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(got_bytes.load(Ordering::SeqCst), 0, "no request byte reached the daemon port");

        let ok = tls_client_get(&cert_pem, https_port, "app.example.k2.dev", "/").await;
        assert!(
            ok.starts_with("HTTP/1.1 200 OK\r\n") && ok.contains("APP"),
            "a normal published app is unaffected, got:\n{ok}"
        );

        subdomains::store(SubdomainMap::default());
    }

    /// App runtime AR4: a deploy's switch is the daemon-local override.
    /// The control-plane target of the label points at a dead port; with
    /// the override set, the TLS splice reaches the App's live backend, and
    /// the (fake) control plane sees zero requests.
    #[tokio::test]
    async fn app_route_override_wins_without_a_control_plane_call() {
        use k2_core::tunnel::subdomains::{self, SubdomainMap};
        use std::sync::atomic::{AtomicUsize, Ordering};

        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let _home = crate::test_support::TempHome::new();
        let daemon_port = install_test_ingress_state();

        // A control plane that counts every connection it gets.
        let hits = Arc::new(AtomicUsize::new(0));
        let cp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let cp_url = format!("http://{}", cp.local_addr().unwrap());
        let h2 = hits.clone();
        std::thread::spawn(move || {
            for s in cp.incoming().flatten() {
                h2.fetch_add(1, Ordering::SeqCst);
                drop(s);
            }
        });
        let _cp = k2_core::test_env::EnvVar::set(subdomains::CONTROL_PLANE_BASE_ENV, &cp_url);

        let live = spawn_tagged_stub("LIVE-SEED").await;
        let closed = k2_core::test_env::ClosedPort::dual();
        let mut targets = std::collections::HashMap::new();
        targets.insert("ovr".to_string(), format!("localhost:{}", closed.port()));
        subdomains::store(SubdomainMap { primary: "example".to_string(), targets });

        let _self_signed = k2_core::test_env::EnvVar::set("K2_E2E_SELF_SIGNED", "1");
        let (cert_pem, key_pem) = k2_core::tunnel::tls::load_or_provision_cert("example").expect("cert");
        let server_config = k2_core::tunnel::tls::server_config(&cert_pem, &key_pem).expect("server config");
        let shared = Arc::new(ArcSwap::from_pointee(server_config));
        let tls_listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind tls");
        let https_port = tls_listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            accept_loop(Arc::new(tls_listener), shared, daemon_port).await;
        });

        let before = tls_client_get(&cert_pem, https_port, "ovr.example.k2.dev", "/").await;
        assert!(before.starts_with("HTTP/1.1 502"), "no override: the dead target answers 502, got:\n{before}");

        crate::apps::router::switch("ovr", live);
        let got = tls_client_get(&cert_pem, https_port, "ovr.example.k2.dev", "/").await;
        assert!(got.contains("LIVE-SEED"), "the override must win over the control-plane target, got:\n{got}");
        assert_eq!(hits.load(Ordering::SeqCst), 0, "a switch never calls the control plane");

        // An unknown label is still a 404, override or not.
        crate::apps::router::switch("nope", live);
        let unknown = tls_client_get(&cert_pem, https_port, "nope.example.k2.dev", "/").await;
        assert!(unknown.contains("404"), "only labels the control plane routes here: {unknown}");

        crate::apps::router::clear("ovr");
        crate::apps::router::clear("nope");
        subdomains::store(SubdomainMap::default());
    }

    // ── 0.46.3: the client IP from frpc's PROXY v2 header ──────────

    /// A TLS session to the E2E listener as frpc opens it: `preamble`
    /// (frpc's PROXY v2 header, or nothing for an old config) and then the
    /// client's TLS bytes.
    async fn frpc_session(
        cert_pem: &str,
        port: u16,
        sni: &str,
        preamble: Option<Vec<u8>>,
    ) -> std::io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
        use rustls_pemfile::Item;
        let mut roots = rustls::RootCertStore::empty();
        for item in rustls_pemfile::read_all(&mut cert_pem.as_bytes()) {
            if let Ok(Item::X509Certificate(der)) = item {
                roots.add(der).expect("add self-signed cert to roots");
            }
        }
        let cfg = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
        let mut tcp = TcpStream::connect(("127.0.0.1", port)).await?;
        if let Some(p) = preamble {
            tcp.write_all(&p).await?;
        }
        let name = rustls::pki_types::ServerName::try_from(sni.to_string()).expect("valid SNI");
        tokio_rustls::TlsConnector::from(Arc::new(cfg)).connect(name, tcp).await
    }

    /// Read until `buf` holds `n` bytes.
    async fn fill_to<S: tokio::io::AsyncRead + Unpin>(s: &mut S, buf: &mut Vec<u8>, n: usize) {
        while buf.len() < n {
            let mut b = [0u8; 4096];
            let k = tokio::time::timeout(Duration::from_secs(10), s.read(&mut b))
                .await
                .expect("bytes in time")
                .expect("read");
            assert!(k > 0, "closed; have {:?}", String::from_utf8_lossy(buf));
            buf.extend_from_slice(&b[..k]);
        }
    }

    /// One `Content-Length` (or 101) response off `s`, keeping leftovers
    /// in `buf`: (head, body).
    async fn next_response<S: tokio::io::AsyncRead + Unpin>(s: &mut S, buf: &mut Vec<u8>) -> (String, String) {
        let end = loop {
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
            let have = buf.len();
            fill_to(s, buf, have + 1).await;
        };
        let head = String::from_utf8_lossy(&buf.drain(..end).collect::<Vec<u8>>()).to_string();
        let len = if head.starts_with("HTTP/1.1 101") {
            0
        } else {
            header_values(&head, "content-length").first().map(|v| v.parse::<usize>().unwrap()).unwrap_or(0)
        };
        fill_to(s, buf, len).await;
        let body = String::from_utf8_lossy(&buf.drain(..len).collect::<Vec<u8>>()).to_string();
        (head, body)
    }

    fn header_values(head: &str, name: &str) -> Vec<String> {
        head.lines()
            .filter_map(|l| {
                let (n, v) = l.split_once(':')?;
                n.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_string())
            })
            .collect()
    }

    const SPOOFED: &str = "X-Forwarded-For: 6.6.6.6\r\nX-Real-IP: 6.6.6.6\r\nForwarded: for=6.6.6.6\r\nX-K2-Client-IP: 6.6.6.6\r\nX_Forwarded_For: 6.6.6.6\r\n";

    /// What the echo App saw must carry exactly K2's headers (`want`) or
    /// none, and never a client copy.
    fn assert_app_saw(seen: &str, want: Option<&str>) {
        assert!(!seen.contains("6.6.6.6"), "a client-sent copy reached the App:\n{seen}");
        assert!(header_values(seen, "forwarded").is_empty(), "{seen}");
        let want: Vec<String> = want.map(|w| vec![w.to_string()]).unwrap_or_default();
        for h in ["x-k2-client-ip", "x-forwarded-for", "x-real-ip"] {
            assert_eq!(header_values(seen, h), want, "{h}:\n{seen}");
        }
    }

    /// 0.46.3 end to end: a fake frpc writes the relay's PROXY v2 header,
    /// then TLS. The App behind its label (the router override) and an
    /// opted-in nested service get exactly one K2-set client IP on every
    /// keep-alive request, with every client copy gone; a WebSocket upgrade
    /// still works. A nested service that didn't opt in, and an App with
    /// `host.clientIp: false`, stay a raw splice exactly as before. No header (old frpc config) or a loopback source (old
    /// relay) → no client-IP headers at all. A malformed header → the
    /// connection is dropped before TLS.
    #[tokio::test]
    async fn frpc_proxy_v2_reaches_apps_as_k2_headers_only() {
        use crate::front443::proxy_v2;
        use k2_core::tunnel::subdomains::{self, SubdomainMap};

        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let _home = crate::test_support::TempHome::new();
        let daemon_port = install_test_ingress_state();

        let app = crate::client_ip::tests::echo_app().await;
        let svc = crate::client_ip::tests::echo_app().await;
        let closed = k2_core::test_env::ClosedPort::dual();
        let mut targets = std::collections::HashMap::new();
        targets.insert("shop".to_string(), format!("localhost:{}", closed.port()));
        targets.insert("svc".to_string(), format!("127.0.0.1:{svc}"));
        targets.insert("rawapp".to_string(), format!("localhost:{}", closed.port()));
        subdomains::store(SubdomainMap { primary: "example".to_string(), targets });
        crate::apps::router::switch("shop", app);
        // `host.clientIp: false`: the escape hatch.
        crate::apps::router::switch_with("rawapp", app, false);

        let _self_signed = k2_core::test_env::EnvVar::set("K2_E2E_SELF_SIGNED", "1");
        let (cert_pem, key_pem) = k2_core::tunnel::tls::load_or_provision_cert("example").expect("cert");
        let server_config = k2_core::tunnel::tls::server_config(&cert_pem, &key_pem).expect("server config");
        let shared = Arc::new(ArcSwap::from_pointee(server_config));
        let tls_listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind tls");
        let https_port = tls_listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            accept_loop(Arc::new(tls_listener), shared, daemon_port).await;
        });

        let relay_saw = |src: &str| Some(proxy_v2::header(src.parse().unwrap(), "127.0.0.1:443".parse().unwrap()));

        // Raw by default (nested service) and by escape hatch (App): the
        // request reaches the App byte for byte, spoofs and all, no K2
        // header, and a non-HTTP/1.x request is not refused.
        for sni in ["svc.example.k2.dev", "rawapp.example.k2.dev"] {
            let mut tls = frpc_session(&cert_pem, https_port, sni, relay_saw("203.0.113.7:51515"))
                .await
                .expect("TLS");
            let req = format!("GET /raw HTTP/1.1\r\nHost: {sni}\r\n{SPOOFED}\r\n");
            tls.write_all(req.as_bytes()).await.unwrap();
            let (head, seen) = next_response(&mut tls, &mut Vec::new()).await;
            assert!(head.starts_with("HTTP/1.1 200 OK"), "{sni}: {head}");
            assert_eq!(seen, req, "{sni}: raw splice, unchanged");
            let mut tls = frpc_session(&cert_pem, https_port, sni, relay_saw("203.0.113.7:51515"))
                .await
                .expect("TLS");
            // The h2 preface reaches the App as is (the rewrite would answer
            // K2's own 400); the echo App echoes what it got.
            tls.write_all(b"PRI * HTTP/2.0\r\n\r\n").await.unwrap();
            let (head, seen) = next_response(&mut tls, &mut Vec::new()).await;
            assert!(head.starts_with("HTTP/1.1 200 OK"), "{sni}: the App answered, not K2: {head}");
            assert_eq!(seen, "PRI * HTTP/2.0\r\n\r\n", "{sni}: raw bytes reached the App");
        }

        // Opt the nested service in: now it is rewritten too.
        k2_core::tunnel::client_ip_labels::set("svc", true).expect("opt in");

        // The App (router override) and the opted-in nested service, keep-alive.
        for sni in ["shop.example.k2.dev", "svc.example.k2.dev"] {
            let mut tls = frpc_session(&cert_pem, https_port, sni, relay_saw("203.0.113.7:51515"))
                .await
                .expect("TLS after frpc's header");
            let mut buf = Vec::new();
            for i in 0..3 {
                let req = format!("GET /k{i} HTTP/1.1\r\nHost: {sni}\r\n{SPOOFED}\r\n");
                tls.write_all(req.as_bytes()).await.unwrap();
                let (head, seen) = next_response(&mut tls, &mut buf).await;
                assert!(head.starts_with("HTTP/1.1 200 OK"), "{sni}: {head}");
                assert!(seen.starts_with(&format!("GET /k{i} ")), "{sni}: {seen}");
                assert_app_saw(&seen, Some("203.0.113.7"));
            }
        }

        // WebSocket through the App: rewritten handshake, then raw.
        let mut tls = frpc_session(&cert_pem, https_port, "shop.example.k2.dev", relay_saw("[2001:db8::7]:4000"))
            .await
            .expect("TLS");
        let hs = format!(
            "GET /ws HTTP/1.1\r\nHost: shop.example.k2.dev\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{SPOOFED}\r\n"
        );
        tls.write_all(hs.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        let (head, _) = next_response(&mut tls, &mut buf).await;
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        let n: usize = header_values(&head, "x-handshake-len")[0].parse().unwrap();
        fill_to(&mut tls, &mut buf, n).await;
        let seen = String::from_utf8_lossy(&buf.drain(..n).collect::<Vec<u8>>()).to_string();
        assert_app_saw(&seen, Some("2001:db8::7"));
        tls.write_all(b"\x81\x02hi").await.unwrap();
        fill_to(&mut tls, &mut buf, 4).await;
        assert_eq!(&buf[..4], b"\x81\x02hi", "raw WebSocket bytes after the 101");

        // Old frpc config (no header) and an old relay (loopback source):
        // spoofs still stripped, no client-IP header at all.
        for (what, pre) in [("no header", None), ("old relay", relay_saw("127.0.0.1:40000"))] {
            let mut tls = frpc_session(&cert_pem, https_port, "shop.example.k2.dev", pre).await.expect("TLS");
            let req = format!("GET / HTTP/1.1\r\nHost: shop.example.k2.dev\r\n{SPOOFED}\r\n");
            tls.write_all(req.as_bytes()).await.unwrap();
            let (_, seen) = next_response(&mut tls, &mut Vec::new()).await;
            assert!(seen.starts_with("GET / "), "{what}: {seen}");
            assert_app_saw(&seen, None);
        }

        // A malformed header (version nibble 1) never reaches TLS.
        let mut bad = relay_saw("203.0.113.7:1").unwrap();
        bad[12] = 0x11;
        let refused = frpc_session(&cert_pem, https_port, "shop.example.k2.dev", Some(bad)).await;
        assert!(refused.is_err(), "a malformed PROXY header must drop the connection");

        // Opting out restores the raw splice on the next connection.
        k2_core::tunnel::client_ip_labels::set("svc", false).expect("opt out");
        let mut tls = frpc_session(&cert_pem, https_port, "svc.example.k2.dev", relay_saw("203.0.113.7:1"))
            .await
            .expect("TLS");
        let req = format!("GET /again HTTP/1.1\r\nHost: svc.example.k2.dev\r\n{SPOOFED}\r\n");
        tls.write_all(req.as_bytes()).await.unwrap();
        let (_, seen) = next_response(&mut tls, &mut Vec::new()).await;
        assert_eq!(seen, req, "opted out: raw again");

        crate::apps::router::clear("shop");
        crate::apps::router::clear("rawapp");
        subdomains::store(SubdomainMap::default());
    }

    // ── H2: slowloris handshake timeout ─────────────────────────────

    /// Slowloris: a client opens the TCP connection to the TLS listener
    /// but NEVER sends any TLS handshake bytes. `serve_one` must give up
    /// after `TLS_HANDSHAKE_TIMEOUT` (not hang forever) and return an
    /// error — proving a half-open peer can't pin a task/permit. We drive
    /// `serve_one` directly with a self-signed acceptor and a live (but
    /// silent) client socket, and assert it completes well within a small
    /// margin of the deadline.
    #[tokio::test]
    async fn slowloris_handshake_times_out_within_deadline() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let _self_signed = k2_core::test_env::EnvVar::set("K2_E2E_SELF_SIGNED", "1");
        // Sandbox $HOME (crate-wide serialized) so the self-signed leaf is
        // minted fresh for "rosson" rather than reusing this box's real cert.
        let _home = crate::test_support::TempHome::new();
        let (cert_pem, key_pem) =
            k2_core::tunnel::tls::load_or_provision_cert("rosson").expect("cert");
        let server_config =
            k2_core::tunnel::tls::server_config(&cert_pem, &key_pem).expect("server config");
        let acceptor = TlsAcceptor::from(Arc::new(server_config));

        // A listener whose accepted socket we hand to serve_one; the client
        // half connects but stays SILENT (never starts the handshake).
        let l = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = l.local_addr().unwrap().port();

        // Keep the silent client alive for the whole test so the server side
        // doesn't see an EOF (which would end the handshake early). It just
        // sits there sending nothing.
        let _silent_client = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("client connect (then stay silent)");

        let (server_tcp, _peer) = l.accept().await.expect("accept");

        // serve_one must error out via the handshake timeout, not hang.
        let started = std::time::Instant::now();
        let res = tokio::time::timeout(
            TLS_HANDSHAKE_TIMEOUT + Duration::from_secs(5),
            serve_one(server_tcp, acceptor, port),
        )
        .await;
        let elapsed = started.elapsed();

        let inner = res.expect("serve_one must return (handshake timeout), not hang past the deadline");
        let err = inner.expect_err("a never-completed handshake must be an Err, not Ok");
        assert!(
            err.contains("timed out"),
            "expected a handshake-timeout error, got: {err}"
        );
        assert!(
            elapsed < TLS_HANDSHAKE_TIMEOUT + Duration::from_secs(3),
            "handshake should have been abandoned ~{TLS_HANDSHAKE_TIMEOUT:?} in, took {elapsed:?}"
        );
    }

    // ── H2: global connection-cap semaphore ─────────────────────────

    /// The process-global connection semaphore is the FD-exhaustion cap:
    /// it must hand out exactly `max_e2e_connections()` permits and then
    /// refuse `try_acquire_owned` (the accept loop drops the connection on
    /// that refusal). Releasing a permit must let the next acquire succeed.
    /// We use a fresh local `Semaphore` of the same size so the assertion
    /// is deterministic regardless of any concurrent test holding permits
    /// on the real process-global one.
    #[test]
    fn connection_cap_semaphore_blocks_past_limit_and_recovers() {
        let sem = Arc::new(Semaphore::new(max_e2e_connections()));

        // Drain every permit.
        let mut held = Vec::new();
        for _ in 0..max_e2e_connections() {
            held.push(
                Arc::clone(&sem)
                    .try_acquire_owned()
                    .expect("each permit up to the cap must be grantable"),
            );
        }
        assert_eq!(sem.available_permits(), 0, "all permits must be taken");

        // One more must be REFUSED (this is the drop-the-connection path).
        assert!(
            Arc::clone(&sem).try_acquire_owned().is_err(),
            "acquiring past the cap must fail so the accept loop drops the conn"
        );

        // Releasing one permit (a connection ending) frees a slot again.
        held.pop();
        assert_eq!(sem.available_permits(), 1, "ending a conn must release its permit");
        assert!(
            Arc::clone(&sem).try_acquire_owned().is_ok(),
            "a freed permit must be re-acquirable"
        );
    }

    /// The shared accessor must always hand back the SAME global semaphore
    /// sized to the cap (so the accept loop and any future caller share one
    /// budget rather than each minting their own).
    #[test]
    fn conn_semaphore_is_a_shared_singleton_sized_to_cap() {
        let a = conn_semaphore();
        let b = conn_semaphore();
        assert!(Arc::ptr_eq(a, b), "conn_semaphore must be a process singleton");
        // Total capacity (available, since nothing is held here in isolation
        // under single-threaded test runs) equals the configured cap.
        assert!(
            a.available_permits() <= max_e2e_connections(),
            "semaphore must never exceed the configured cap"
        );
    }

    // ── H7: ALPN is http/1.1 ONLY (no h2 over the byte-splice) ───────

    /// Defence-in-depth duplicate of the k2-core assertion: the rustls
    /// `ServerConfig` this listener terminates with MUST advertise ONLY
    /// `http/1.1`. Advertising h2 made browsers negotiate HTTP/2, whose
    /// frames break the raw HTTP/1.1 byte-splice (ERR_HTTP2_PROTOCOL_ERROR).
    /// Fixed in 0.40.6; asserted here so a regression in `server_config`
    /// fails a daemon test, not just a core test.
    #[test]
    fn server_config_advertises_only_http1_1() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let _self_signed = k2_core::test_env::EnvVar::set("K2_E2E_SELF_SIGNED", "1");
        // Sandbox $HOME (crate-wide serialized) so the self-signed leaf is
        // minted fresh for "rosson" rather than reusing this box's real cert.
        let _home = crate::test_support::TempHome::new();
        let (cert_pem, key_pem) =
            k2_core::tunnel::tls::load_or_provision_cert("rosson").expect("cert");
        let cfg =
            k2_core::tunnel::tls::server_config(&cert_pem, &key_pem).expect("server config");
        assert_eq!(
            cfg.alpn_protocols,
            vec![b"http/1.1".to_vec()],
            "E2E listener must advertise only http/1.1 (raw-splice backend; no h2)"
        );
    }

    // ── H6: clean close_notify on the Daemon arm ────────────────────

    /// After the spliced backend closes, `serve_one`'s Daemon arm calls
    /// `tls.shutdown()`, which emits a TLS `close_notify`. A strict rustls
    /// client therefore sees an ORDERLY close (read_to_end succeeds with no
    /// `UnexpectedEof`), not a truncated stream. We stand up the real
    /// accept loop against a stub backend that returns a `Connection: close`
    /// response and assert the client read terminates cleanly.
    #[tokio::test]
    async fn daemon_arm_emits_close_notify_on_clean_close() {
        use rustls_pemfile::Item;
        use tokio_rustls::TlsConnector;

        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let _self_signed = k2_core::test_env::EnvVar::set("K2_E2E_SELF_SIGNED", "1");
        // Redirect $HOME to a fresh tempdir so the self-signed cert is minted
        // for OUR chosen name ("rosson") rather than reusing whatever leaf is
        // persisted in the real ~/.k2 on this box (which may be for another
        // subdomain). `load_or_provision_cert` honors $HOME via `k2_dir()`.
        // Serialized on the ONE shared env lock so concurrent
        // HOME-swapping tests in other modules can't stomp this sandbox.
        let _home = crate::test_support::TempHome::new();

        // No nested subdomains → everything routes to the daemon arm.
        use k2_core::tunnel::subdomains::{self, SubdomainMap};
        subdomains::store(SubdomainMap {
            primary: "rosson".to_string(),
            targets: std::collections::HashMap::new(),
        });

        // Daemon arm → real dispatcher via the tunnel-ingress listener
        // (`backend_port` is the never-listening main port).
        let backend_port = install_test_ingress_state();

        let (cert_pem, key_pem) =
            k2_core::tunnel::tls::load_or_provision_cert("rosson").expect("cert");
        let server_config =
            k2_core::tunnel::tls::server_config(&cert_pem, &key_pem).expect("server config");
        let shared = Arc::new(ArcSwap::from_pointee(server_config));

        let tls_listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind tls");
        let https_port = tls_listener.local_addr().unwrap().port();
        {
            let shared = shared.clone();
            tokio::spawn(async move {
                accept_loop(Arc::new(tls_listener), shared, backend_port).await;
            });
        }

        // Build a client that trusts the self-signed leaf and reads to EOF.
        let mut roots = rustls::RootCertStore::empty();
        for item in rustls_pemfile::read_all(&mut cert_pem.as_bytes()) {
            if let Ok(Item::X509Certificate(der)) = item {
                roots.add(der).expect("add cert");
            }
        }
        let client_config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(client_config));
        let tcp = TcpStream::connect(("127.0.0.1", https_port))
            .await
            .expect("connect");
        let sni = rustls::pki_types::ServerName::try_from("rosson.k2.dev".to_string())
            .expect("sni");
        let mut tls = connector.connect(sni, tcp).await.expect("client handshake");

        tls.write_all(b"GET /ping HTTP/1.1\r\nHost: rosson.k2.dev\r\nConnection: close\r\n\r\n")
            .await
            .expect("write");
        tls.flush().await.expect("flush");

        // A clean close_notify means read_to_end returns Ok (not an
        // UnexpectedEof / CloseNotify error). The body must be present too.
        let mut buf = Vec::new();
        let read = tls.read_to_end(&mut buf).await;
        let text = String::from_utf8_lossy(&buf).into_owned();
        assert!(
            read.is_ok(),
            "clean close_notify expected; read_to_end errored: {:?}",
            read.err()
        );
        assert!(
            text.contains("200 OK") && text.contains(crate::BANNER),
            "expected the spliced 200 response, got:\n{text}"
        );

        subdomains::store(SubdomainMap::default());
        // `_home` (TempHome) restores $HOME and removes the tempdir on drop.
    }

    // ── 0.46.2: a failed renewal never takes a valid cert out of service ──

    /// A loopback cert broker that answers each request with the next
    /// scripted `(status line, body)` (the last one repeats) and counts the
    /// requests it served.
    struct FakeBroker {
        url: String,
        hits: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl FakeBroker {
        fn start(script: Vec<(&'static str, String)>) -> Self {
            use std::io::{Read, Write};
            let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind fake broker");
            let port = l.local_addr().expect("addr").port();
            let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let h = hits.clone();
            std::thread::spawn(move || {
                for sock in l.incoming() {
                    let Ok(mut sock) = sock else { return };
                    // Read the whole request (headers + Content-Length body).
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        let n = sock.read(&mut chunk).unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&buf).to_string();
                        if let Some(end) = text.find("\r\n\r\n") {
                            let len = text[..end]
                                .lines()
                                .find_map(|l| {
                                    let (k, v) = l.split_once(':')?;
                                    k.eq_ignore_ascii_case("content-length")
                                        .then(|| v.trim().parse::<usize>().ok())
                                        .flatten()
                                })
                                .unwrap_or(0);
                            if buf.len() >= end + 4 + len {
                                break;
                            }
                        }
                    }
                    let i = h.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let (status, body) = script
                        .get(i)
                        .or_else(|| script.last())
                        .cloned()
                        .expect("non-empty script");
                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes());
                    let _ = sock.flush();
                }
            });
            Self { url: format!("http://127.0.0.1:{port}/cert"), hits }
        }

        fn hits(&self) -> usize {
            self.hits.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    fn broker_issuance_failed() -> (&'static str, String) {
        (
            "502 Bad Gateway",
            serde_json::json!({
                "error": "issuance_failed",
                "detail": "order did not become ready (status: Invalid)"
            })
            .to_string(),
        )
    }

    fn broker_ok(cert_pem: &str) -> (&'static str, String) {
        ("200 OK", serde_json::json!({ "cert": cert_pem }).to_string())
    }

    /// A cert for `sub` signed by the daemon's persisted tunnel key, valid
    /// until `days` from today (negative = already expired).
    fn cert_signed_by_tunnel_key(sub: &str, days: i64) -> String {
        use chrono::Datelike;
        let kp = k2_core::tunnel::tls::load_or_generate_keypair().expect("tunnel keypair");
        let mut params =
            rcgen::CertificateParams::new(k2_core::tunnel::tls::sans_for(sub)).expect("params");
        let ymd = |d: chrono::DateTime<chrono::Utc>| {
            rcgen::date_time_ymd(d.year(), d.month() as u8, d.day() as u8)
        };
        let now = chrono::Utc::now();
        params.not_before = ymd(now - chrono::Duration::days(80));
        params.not_after = ymd(now + chrono::Duration::days(days));
        params.self_signed(&kp).expect("sign").pem()
    }

    /// TLS-handshake 127.0.0.1:`port` for `sni`, trusting only `trust_pem`,
    /// and return the leaf the server presented (DER). Fails loudly when
    /// the handshake fails (the served cert is not `trust_pem`).
    async fn served_leaf(trust_pem: &str, port: u16, sni: &str) -> Vec<u8> {
        use tokio_rustls::TlsConnector;
        let mut roots = rustls::RootCertStore::empty();
        for item in rustls_pemfile::certs(&mut trust_pem.as_bytes()) {
            roots.add(item.expect("trust pem")).expect("add root");
        }
        let cfg = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let tcp = TcpStream::connect(("127.0.0.1", port)).await.expect("connect");
        let name = rustls::pki_types::ServerName::try_from(sni.to_string()).expect("sni");
        let tls = TlsConnector::from(Arc::new(cfg))
            .connect(name, tcp)
            .await
            .expect("TLS handshake against the expected cert");
        let (_, conn) = tls.get_ref();
        conn.peer_certificates().expect("peer certs")[0].as_ref().to_vec()
    }

    fn leaf_der(pem: &str) -> Vec<u8> {
        rustls_pemfile::certs(&mut pem.as_bytes())
            .next()
            .expect("a cert")
            .expect("parse cert")
            .as_ref()
            .to_vec()
    }

    /// Seed a connectable E2E config for `rosson` and point the broker at
    /// `url` (spike hatch off, E2E default on, air-gap off).
    fn seed_e2e_config(url: &str) -> Vec<k2_core::test_env::EnvVar> {
        let guards = vec![
            k2_core::test_env::EnvVar::remove("K2_E2E_SELF_SIGNED"),
            k2_core::test_env::EnvVar::remove("K2_E2E"),
            k2_core::test_env::EnvVar::remove("K2_AIRGAP"),
            k2_core::test_env::EnvVar::set(k2_core::tunnel::cert_broker::BROKER_URL_ENV, url),
        ];
        k2_core::tunnel::config::save(&k2_core::tunnel::config::TunnelConfig {
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            ..Default::default()
        })
        .expect("seed tunnel.json");
        guards
    }

    async fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !cond() {
            assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn fast_schedule() -> RenewSchedule {
        RenewSchedule {
            check_every: Duration::from_secs(3600),
            retry_after_failure: vec![Duration::from_millis(50)],
        }
    }

    #[test]
    fn renew_schedule_production_backs_off_to_hourly() {
        let s = RenewSchedule::production();
        assert_eq!(s.check_every, Duration::from_secs(12 * 60 * 60));
        let got: Vec<u64> = (1..=8).map(|n| s.retry_delay(n).as_secs()).collect();
        assert_eq!(got, vec![60, 120, 300, 900, 1800, 3600, 3600, 3600]);
    }

    /// (a) The 2026-10-19 outage: the installed cert is valid but inside
    /// the renewal window and the broker fails. The listener must start on
    /// the OLD cert, the renewer must retry on its backoff, and once the
    /// broker issues, the new cert is hot-swapped in.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn valid_cert_in_window_starts_listener_and_renewal_retries() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let _home = crate::test_support::TempHome::new();
        let old = cert_signed_by_tunnel_key("rosson", 10);
        let fresh = cert_signed_by_tunnel_key("rosson", 90);
        k2_core::tunnel::cert_broker::install_cert(&old).expect("install old cert");
        let broker = FakeBroker::start(vec![
            broker_issuance_failed(),
            broker_issuance_failed(),
            broker_ok(&fresh),
        ]);
        let _env = seed_e2e_config(&broker.url);
        retire_any();

        let port = spawn_listener_with(1, "rosson".to_string(), fast_schedule())
            .await
            .expect("a valid cert in the renewal window must start the listener");
        assert_eq!(
            live_renewer_subdomain().as_deref(),
            Some("rosson"),
            "the renewer must run even though the broker is failing"
        );
        // The renewer tries at once, fails twice, then the broker issues.
        wait_for("three broker requests (2 failures, then success)", || broker.hits() >= 3).await;
        wait_for("the renewed cert on disk", || {
            std::fs::read_to_string(k2_core::tunnel::tls::cert_path()).ok().as_deref()
                == Some(fresh.as_str())
        })
        .await;
        // The hot-swap lands right after the install.
        let mut swapped = false;
        for _ in 0..100 {
            let tcp_ok = std::net::TcpStream::connect(("127.0.0.1", port)).is_ok();
            assert!(tcp_ok, "the listener must stay up through the renewal");
            if tokio::time::timeout(
                Duration::from_secs(5),
                served_leaf_or_none(&fresh, port, "rosson.k2.dev"),
            )
            .await
            .expect("handshake timeout")
                == Some(leaf_der(&fresh))
            {
                swapped = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(swapped, "the renewed cert must be hot-swapped into the live listener");
        assert_eq!(broker.hits(), 3, "a fresh cert must not be re-requested");
        retire_any();
    }

    /// Before the broker recovers, the listener serves the OLD cert.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn old_cert_is_served_while_renewal_fails() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let _home = crate::test_support::TempHome::new();
        let old = cert_signed_by_tunnel_key("rosson", 10);
        k2_core::tunnel::cert_broker::install_cert(&old).expect("install old cert");
        let broker = FakeBroker::start(vec![broker_issuance_failed()]);
        let _env = seed_e2e_config(&broker.url);
        retire_any();

        let port = spawn_listener_with(1, "rosson".to_string(), fast_schedule())
            .await
            .expect("listener starts on the old cert");
        wait_for("at least two renewal attempts (retry scheduled)", || broker.hits() >= 2).await;
        assert_eq!(
            served_leaf(&old, port, "rosson.k2.dev").await,
            leaf_der(&old),
            "the still-valid old cert must keep serving while renewal fails"
        );
        assert_eq!(live_renewer_subdomain().as_deref(), Some("rosson"));
        retire_any();
    }

    /// (b) Expired cert + broker failure: nothing to serve, so no start;
    /// the auto-start retry driver (the production one, with short waits)
    /// keeps trying through the real ensure path until the broker issues,
    /// then the listener starts on the new cert.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn expired_cert_no_start_then_retry_starts_when_broker_recovers() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let _home = crate::test_support::TempHome::new();
        let expired = cert_signed_by_tunnel_key("rosson", -2);
        let fresh = cert_signed_by_tunnel_key("rosson", 90);
        k2_core::tunnel::cert_broker::install_cert(&expired).expect("install expired cert");
        let broker = FakeBroker::start(vec![
            broker_issuance_failed(),
            broker_issuance_failed(),
            broker_ok(&fresh),
        ]);
        let _env = seed_e2e_config(&broker.url);
        retire_any();

        let handle = tokio::runtime::Handle::current();
        let attempt = || {
            let handle = handle.clone();
            async move {
                tokio::task::spawn_blocking(move || ensure_spawned_blocking(handle, 1))
                    .await
                    .expect("ensure task")
                    .map(|p| format!("listener on {p}"))
            }
        };
        let steps = [Duration::from_millis(20)];
        let outcome = crate::tunnel_autostart::retry_until_started(&steps, attempt, || {
            crate::tunnel_autostart::Gate::Go
        })
        .await;
        let crate::tunnel_autostart::RetryOutcome::Started { attempts, .. } = outcome else {
            panic!("expected the retry to start the listener, got {outcome:?}");
        };
        assert_eq!(attempts, 3, "two failed starts (no usable cert), then success");
        assert_eq!(broker.hits(), 3);
        let (port, sub) = live_listener().expect("listener recorded");
        assert_eq!(sub, "rosson");
        assert_eq!(
            served_leaf(&fresh, port, "rosson.k2.dev").await,
            leaf_der(&fresh),
            "the listener must serve the newly issued cert"
        );
        retire_any();
    }

    /// The first failed attempt in (b) really is "no start": an expired
    /// cert with a failing broker returns an error and records nothing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn expired_cert_and_broker_failure_does_not_start() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let _home = crate::test_support::TempHome::new();
        let expired = cert_signed_by_tunnel_key("rosson", -2);
        k2_core::tunnel::cert_broker::install_cert(&expired).expect("install expired cert");
        let broker = FakeBroker::start(vec![broker_issuance_failed()]);
        let _env = seed_e2e_config(&broker.url);
        retire_any();

        let err = spawn_listener_with(1, "rosson".to_string(), fast_schedule())
            .await
            .expect_err("an expired cert must not be served");
        assert!(err.contains("issuance_failed"), "expected the broker error, got: {err}");
        assert!(live_listener().is_none(), "no listener may be recorded");
        assert_eq!(broker.hits(), 1);
    }

    async fn served_leaf_or_none(trust_pem: &str, port: u16, sni: &str) -> Option<Vec<u8>> {
        use tokio_rustls::TlsConnector;
        let mut roots = rustls::RootCertStore::empty();
        for item in rustls_pemfile::certs(&mut trust_pem.as_bytes()) {
            roots.add(item.expect("trust pem")).expect("add root");
        }
        let cfg = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let tcp = TcpStream::connect(("127.0.0.1", port)).await.expect("connect");
        let name = rustls::pki_types::ServerName::try_from(sni.to_string()).expect("sni");
        let tls = TlsConnector::from(Arc::new(cfg)).connect(name, tcp).await.ok()?;
        let (_, conn) = tls.get_ref();
        Some(conn.peer_certificates()?[0].as_ref().to_vec())
    }
}
