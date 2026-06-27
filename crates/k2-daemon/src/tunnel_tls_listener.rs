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

/// The live HTTPS-listener port, set the first time the listener binds
/// (boot `maybe_spawn` OR on-demand `ensure_spawned`). Makes spawning
/// idempotent: a second ensure/spawn returns this rather than binding a
/// duplicate listener. `None` until the listener is up.
static HTTPS_PORT: OnceLock<u16> = OnceLock::new();

/// Serializes concurrent on-demand spawns so two simultaneous tunnel starts
/// can't race into two listeners. Cheap — held only across the bind, not the
/// accept loop.
static SPAWN_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn spawn_lock() -> &'static Mutex<()> {
    SPAWN_LOCK.get_or_init(|| Mutex::new(()))
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

/// Process-global cap on concurrently-served E2E TLS connections. Each
/// accepted connection takes one permit BEFORE its task is spawned and
/// holds it for the connection's life (handshake + splice), so the
/// listener can't be driven to FD exhaustion / unbounded task growth by a
/// flood of connections. A few hundred is ample for a single-user remote
/// daemon (real concurrency is a handful of browser tabs + WS streams)
/// while still leaving plenty of FD headroom under the default rlimit.
const MAX_E2E_CONNECTIONS: usize = 256;

/// The global E2E connection semaphore (lazily created, lives for the
/// process). Shared by every accept loop (there is normally exactly one).
fn conn_semaphore() -> &'static Arc<Semaphore> {
    static SEM: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEM.get_or_init(|| Arc::new(Semaphore::new(MAX_E2E_CONNECTIONS)))
}

/// Spawn the E2E HTTPS listener when the `K2_E2E` flag is on. Returns the
/// chosen HTTPS port (for the tunnel connector / logging) or `None` when
/// E2E is disabled. Errors are returned (caller logs + continues — a failed
/// E2E listener must never take the daemon down).
///
/// `http_port` is the daemon's existing cleartext HTTP listener port — the
/// decrypted stream is spliced there so the unmodified dispatcher serves it.
pub async fn maybe_spawn(http_port: u16) -> Result<Option<u16>, String> {
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

    spawn_listener(http_port, subdomain).await.map(Some)
}

/// Bind + spawn the E2E HTTPS listener for `subdomain`, provisioning its cert
/// via the broker seam first, and record the chosen port in [`HTTPS_PORT`].
/// Idempotent guard lives in the callers; this is the actual bring-up.
async fn spawn_listener(http_port: u16, subdomain: String) -> Result<u16, String> {
    // Cert + key via the broker seam (broker-issued on the real path;
    // self-signed only under the explicit `K2_E2E_SELF_SIGNED` spike hatch).
    // On the real path a broker-unreachable failure surfaces HERE and is
    // returned — the caller logs it and the tunnel does NOT come up against
    // an untrusted/missing cert (no silent cleartext fallback).
    let (cert_pem, key_pem) =
        k2_core::tunnel::tls::load_or_provision_cert(&subdomain)
            .map_err(|e| format!("provision tunnel cert: {e}"))?;
    let server_config = k2_core::tunnel::tls::server_config(&cert_pem, &key_pem)
        .map_err(|e| format!("build rustls ServerConfig: {e}"))?;

    // Hold the live ServerConfig behind an ArcSwap so the renewal loop can
    // HOT-SWAP a freshly-issued chain in WITHOUT dropping the listener: each
    // new connection reads the current config, so a swap takes effect on the
    // next handshake with zero downtime.
    let shared_config: Arc<ArcSwap<ServerConfig>> =
        Arc::new(ArcSwap::from_pointee(server_config));

    // Bind an ephemeral loopback port for the HTTPS listener (mirrors the
    // HTTP listener pattern). 127.0.0.1 only — frpc dials it locally.
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| format!("bind E2E HTTPS listener: {e}"))?;
    let https_port = listener
        .local_addr()
        .map_err(|e| format!("read HTTPS listener addr: {e}"))?
        .port();

    // Record the live port so concurrent/later ensure calls are idempotent
    // (return this port instead of binding a second listener). First writer
    // wins; we're inside SPAWN_LOCK on the on-demand path so there's no race.
    let _ = HTTPS_PORT.set(https_port);

    // Publish the port so the tunnel connector points frpc's `localPort`
    // here (and not at the cleartext HTTP port).
    k2_core::tunnel::tls::publish_https_port(https_port)
        .map_err(|e| format!("publish HTTPS port: {e}"))?;

    log_debug!(
        "[daemon/e2e] K2 Connect E2E TLS listener up on 127.0.0.1:{} \
         (subdomain {}.k2.dev; decrypted → HTTP :{})",
        https_port,
        subdomain,
        http_port
    );

    {
        let cfg = shared_config.clone();
        tokio::spawn(async move {
            accept_loop(listener, cfg, http_port).await;
        });
    }

    // Auto-renewal (PRD §6): re-issue ~30d pre-expiry and hot-swap the
    // config. Skipped under the self-signed spike hatch (a self-signed cert
    // has no broker to renew against; the spike is a local dev convenience).
    if !k2_core::tunnel::cert_broker::self_signed_mode() {
        let cfg = shared_config.clone();
        let sub = subdomain.clone();
        tokio::spawn(async move {
            cert_renewal_loop(sub, cfg).await;
        });
    }

    Ok(https_port)
}

/// On-demand bring-up of the E2E HTTPS listener, callable from a BLOCKING
/// context (the tunnel connector's `spawn_blocking` worker). This is the
/// robust first-connect path now that E2E is the default: a tunnel start
/// ENSURES the listener exists — provisioning the cert + binding if needed —
/// before frpc forwards to it, instead of erroring when boot never published
/// a port (e.g. the subdomain was configured after the daemon booted).
///
/// Idempotent + serialized:
///   * if a listener is already up, returns its recorded port immediately;
///   * otherwise takes [`spawn_lock`], re-checks (double-checked locking),
///     and drives the async [`spawn_listener`] to completion on the provided
///     tokio runtime `handle` (we're on a blocking thread, so `block_on`).
///
/// Errors loud — issuance/bind failure or a still-empty subdomain returns an
/// `Err` the connector propagates; it never silently forwards cleartext.
fn ensure_spawned_blocking(handle: tokio::runtime::Handle, http_port: u16) -> Result<u16, String> {
    // Fast path: already running.
    if let Some(p) = HTTPS_PORT.get() {
        return Ok(*p);
    }

    let _g = spawn_lock().lock().unwrap_or_else(|p| p.into_inner());
    // Re-check under the lock — another start may have spawned it while we
    // waited for the lock.
    if let Some(p) = HTTPS_PORT.get() {
        return Ok(*p);
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

    // Drive the async bind/provision/spawn to completion from this blocking
    // thread. `block_on` on a borrowed Handle is valid from a non-runtime
    // thread (the connector calls us inside `spawn_blocking`).
    handle.block_on(spawn_listener(http_port, subdomain))
}

/// Install the k2-core seam so the (k2-core) tunnel connector can demand this
/// (k2-daemon) listener on first connect without a layering inversion. Call
/// once at boot, AFTER a tokio runtime exists — we capture its [`Handle`] so
/// the blocking hook can drive the async bring-up. See
/// [`k2_core::tunnel::tls::register_ensure_https_listener`].
pub fn register_ensure_hook() {
    let handle = tokio::runtime::Handle::current();
    k2_core::tunnel::tls::register_ensure_https_listener(Box::new(move |http_port| {
        ensure_spawned_blocking(handle.clone(), http_port)
    }));
}

/// Accept TLS connections forever, handing each to a per-connection task.
/// Reads the CURRENT [`ServerConfig`] from `shared_config` on every accept
/// so a renewal swap is picked up by the next connection with no listener
/// restart.
async fn accept_loop(
    listener: TcpListener,
    shared_config: Arc<ArcSwap<ServerConfig>>,
    http_port: u16,
) {
    loop {
        match listener.accept().await {
            Ok((tcp, _peer)) => {
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
                            "[daemon/e2e] connection cap ({MAX_E2E_CONNECTIONS}) reached \
                             — dropping inbound connection from {_peer}"
                        );
                        drop(tcp); // close the FD now; do not spawn.
                        continue;
                    }
                };
                // Snapshot the live config for THIS connection (cheap Arc
                // clone). A concurrent hot-swap only affects later accepts.
                let acceptor = TlsAcceptor::from(shared_config.load_full());
                tokio::spawn(async move {
                    // Hold the permit for the life of the connection; it is
                    // released when this task (and thus `_permit`) drops.
                    let _permit = permit;
                    if let Err(e) = serve_one(tcp, acceptor, http_port).await {
                        log_debug!("[daemon/e2e] connection ended: {e}");
                    }
                });
            }
            Err(e) => {
                log_debug!("[daemon/e2e] accept error: {e}");
            }
        }
    }
}

/// Daemon-owned cert-renewal loop (mirrors `tunnel::lease`'s daemon-side
/// scheduling). Periodically checks the installed leaf's `notAfter`; when
/// it's within [`k2_core::tunnel::cert_broker::RENEWAL_WINDOW`] of expiry,
/// re-runs the broker client and hot-swaps the rustls config in place.
///
/// Fully fault-isolated: a transient broker failure is logged and retried on
/// the next tick (the OLD cert keeps serving until the new one lands), so a
/// hiccup never drops the listener. Runs for the life of the daemon (the
/// E2E listener has no separate stop signal — it lives as long as the
/// process, like the cleartext listener).
async fn cert_renewal_loop(subdomain: String, shared_config: Arc<ArcSwap<ServerConfig>>) {
    use k2_core::tunnel::cert_broker;

    log_debug!(
        "[daemon/e2e] cert auto-renewal started for {}.k2.dev (check every {:?}, \
         renew within {:?} of expiry)",
        subdomain,
        CERT_CHECK_INTERVAL,
        cert_broker::RENEWAL_WINDOW
    );

    loop {
        tokio::time::sleep(CERT_CHECK_INTERVAL).await;

        // Read the installed cert to decide whether we're inside the window.
        let cert_path = k2_core::tunnel::tls::cert_path();
        let installed = match std::fs::read_to_string(&cert_path) {
            Ok(s) => s,
            Err(e) => {
                log_debug!("[daemon/e2e] renewal: cannot read {}: {e}", cert_path.display());
                continue;
            }
        };
        if !cert_broker::cert_needs_renewal(
            &installed,
            cert_broker::RENEWAL_WINDOW,
            cert_broker::now_unix(),
        ) {
            continue; // still fresh — quiet on the happy path.
        }

        log_debug!(
            "[daemon/e2e] cert for {}.k2.dev is within the renewal window — \
             requesting a fresh chain from the broker",
            subdomain
        );

        // Re-provision via the broker. Blocking reqwest under the hood, so
        // run it off the async runtime. A failure is transient: log + retry.
        let sub = subdomain.clone();
        let provisioned =
            tokio::task::spawn_blocking(move || cert_broker::provision_via_broker(&sub))
                .await;
        let (cert_pem, key_pem) = match provisioned {
            Ok(Ok(pair)) => pair,
            Ok(Err(e)) => {
                log_debug!(
                    "[daemon/e2e] renewal: broker request failed (will retry next tick): {e}"
                );
                continue;
            }
            Err(e) => {
                log_debug!("[daemon/e2e] renewal: provision task panicked: {e}");
                continue;
            }
        };

        match k2_core::tunnel::tls::server_config(&cert_pem, &key_pem) {
            Ok(new_cfg) => {
                shared_config.store(Arc::new(new_cfg));
                log_debug!(
                    "[daemon/e2e] cert for {}.k2.dev renewed + hot-swapped (no listener restart)",
                    subdomain
                );
            }
            Err(e) => log_debug!(
                "[daemon/e2e] renewal: fresh cert didn't build a ServerConfig \
                 (keeping the old one): {e}"
            ),
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
///     `localhost:3000`);
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
/// Generic over anything `TcpStream::connect` accepts (a `(host, port)`
/// tuple for the daemon path, a `&str` "host:port" for nested targets).
async fn connect_upstream<A: tokio::net::ToSocketAddrs>(addr: &A) -> Result<TcpStream, String> {
    match tokio::time::timeout(UPSTREAM_CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
        Ok(Ok(s)) => Ok(s),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!(
            "upstream connect timed out after {UPSTREAM_CONNECT_TIMEOUT:?}"
        )),
    }
}

async fn serve_one(
    tcp: TcpStream,
    acceptor: TlsAcceptor,
    http_port: u16,
) -> Result<(), String> {
    use k2_core::tunnel::subdomains::{self, Route};
    use tokio::io::AsyncWriteExt;

    // H2: bound the TLS handshake. A peer that opens the connection but
    // never finishes the handshake (slowloris) would otherwise pin this
    // task — and its connection permit — indefinitely. The timeout frees
    // both deterministically.
    let mut tls = match tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await {
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
            // Existing path: splice to the daemon's own cleartext HTTP
            // listener; the dispatcher's keep-alive loop owns request framing.
            // H2: bound the upstream dial (loopback is ~instant; a wedged
            // backend must not hang this task).
            let mut upstream = connect_upstream(&("127.0.0.1", http_port))
                .await
                .map_err(|e| format!("connect to local HTTP listener :{http_port}: {e}"))?;
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
            // Configured nested subdomain → proxy the decrypted stream to the
            // user's chosen internal endpoint on this same machine.
            // H2: bound the upstream dial.
            let mut upstream = connect_upstream(&target.as_str()).await.map_err(|e| {
                format!("connect to internal endpoint {target} for SNI {sni}: {e}")
            })?;
            log_debug!("[daemon/e2e] routing {sni} → internal endpoint {target}");
            let res = tokio::io::copy_bidirectional(&mut tls, &mut upstream)
                .await
                .map(|_| ())
                .map_err(|e| format!("splice TLS↔internal {target}: {e}"));
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
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
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
        let prev = std::env::var_os("K2_E2E");
        std::env::set_var("K2_E2E", "0"); // explicit opt-out wins → OFF
        let res = maybe_spawn(12345)
            .await
            .expect("opt-out must not error");
        assert!(res.is_none(), "opt-out must yield no HTTPS listener");
        match prev {
            Some(p) => std::env::set_var("K2_E2E", p),
            None => std::env::remove_var("K2_E2E"),
        }
    }

    /// End-to-end: with a self-signed cert, the HTTPS listener accepts a
    /// real TLS connection and routes a request through the dispatcher
    /// (proven by a live `/ping` 200 over TLS). Exercises the full path —
    /// rustls handshake → decrypt → splice → dispatch — without the env
    /// flag (we call the internals directly so the test is hermetic).
    #[tokio::test]
    async fn https_listener_routes_a_request_through_the_dispatcher() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

        // 1) Stand up a minimal cleartext HTTP "dispatcher" that answers
        //    /ping with 200 — standing in for the daemon's real dispatch()
        //    (we're testing the TLS→splice path, not re-testing dispatch).
        let http = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind http stub");
        let http_port = http.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = match http.accept().await {
                    Ok(v) => v,
                    Err(_) => return,
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let body = if req.starts_with("GET /ping") {
                        "pong"
                    } else {
                        "no"
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = s.write_all(resp.as_bytes()).await;
                    let _ = s.flush().await;
                });
            }
        });

        // 2) Provision a self-signed cert + build the rustls acceptor. This
        //    test exercises the TLS→splice path, not the broker, so engage
        //    the explicit dev/spike self-signed escape hatch
        //    (`K2_E2E_SELF_SIGNED=1`) — the default path is broker-issued.
        std::env::set_var("K2_E2E_SELF_SIGNED", "1");
        // Sandbox $HOME (crate-wide serialized) so the self-signed leaf is
        // minted fresh for "rosson" into a throwaway `.k2/` rather than reusing
        // whatever real cert is persisted in this box's `~/.k2`.
        let _home = crate::test_support::TempHome::new();
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
                accept_loop(tls_listener, shared, http_port).await;
            });
        }

        // 4) Connect a rustls CLIENT that trusts our self-signed cert and
        //    send GET /ping; assert we get the 200/pong back through the
        //    whole TLS→splice→stub-dispatcher path.
        let body = tls_client_get(&cert_pem, https_port, "rosson.k2.dev", "/ping").await;
        assert!(
            body.contains("200 OK") && body.contains("pong"),
            "expected a routed 200 pong over TLS, got:\n{body}"
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

    /// SNI Host routing (PRD §7): the primary `<sub>.k2.dev` reaches the
    /// daemon stub; a CONFIGURED nested label reaches its internal endpoint;
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

        // Two distinct backends so we can prove WHERE a connection landed.
        let daemon_port = spawn_tagged_stub("DAEMON").await;
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
        std::env::set_var("K2_E2E_SELF_SIGNED", "1");
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
                accept_loop(tls_listener, shared, daemon_port).await;
            });
        }

        // Primary → daemon stub.
        let primary = tls_client_get(&cert_pem, https_port, "rosson.k2.dev", "/x").await;
        assert!(
            primary.contains("200 OK") && primary.contains("DAEMON"),
            "primary host must reach the daemon, got:\n{primary}"
        );

        // Configured nested → internal endpoint.
        let nested = tls_client_get(&cert_pem, https_port, "staging.rosson.k2.dev", "/x").await;
        assert!(
            nested.contains("200 OK") && nested.contains("INTERNAL"),
            "configured nested label must reach its internal endpoint, got:\n{nested}"
        );

        // Unknown nested → 404 from the listener itself (NOT the daemon).
        let unknown = tls_client_get(&cert_pem, https_port, "ghost.rosson.k2.dev", "/x").await;
        assert!(
            unknown.contains("404 Not Found") && !unknown.contains("DAEMON"),
            "unknown nested label must be rejected 404, not routed to the daemon, got:\n{unknown}"
        );

        // Reset the global cache so we don't bleed into sibling tests.
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
        std::env::set_var("K2_E2E_SELF_SIGNED", "1");
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
    /// it must hand out exactly `MAX_E2E_CONNECTIONS` permits and then
    /// refuse `try_acquire_owned` (the accept loop drops the connection on
    /// that refusal). Releasing a permit must let the next acquire succeed.
    /// We use a fresh local `Semaphore` of the same size so the assertion
    /// is deterministic regardless of any concurrent test holding permits
    /// on the real process-global one.
    #[test]
    fn connection_cap_semaphore_blocks_past_limit_and_recovers() {
        let sem = Arc::new(Semaphore::new(MAX_E2E_CONNECTIONS));

        // Drain every permit.
        let mut held = Vec::new();
        for _ in 0..MAX_E2E_CONNECTIONS {
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
            a.available_permits() <= MAX_E2E_CONNECTIONS,
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
        std::env::set_var("K2_E2E_SELF_SIGNED", "1");
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
        std::env::set_var("K2_E2E_SELF_SIGNED", "1");
        // Redirect $HOME to a fresh tempdir so the self-signed cert is minted
        // for OUR chosen name ("rosson") rather than reusing whatever leaf is
        // persisted in the real ~/.k2 on this box (which may be for another
        // subdomain). `load_or_provision_cert` honors $HOME via `k2_dir()`.
        // Crate-wide serialized via the shared `home_lock` so concurrent
        // HOME-swapping tests in other modules can't stomp this sandbox.
        let _home = crate::test_support::TempHome::new();

        // No nested subdomains → everything routes to the daemon arm.
        use k2_core::tunnel::subdomains::{self, SubdomainMap};
        subdomains::store(SubdomainMap {
            primary: "rosson".to_string(),
            targets: std::collections::HashMap::new(),
        });

        let backend_port = spawn_tagged_stub("OK").await;

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
                accept_loop(tls_listener, shared, backend_port).await;
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

        tls.write_all(b"GET /x HTTP/1.1\r\nHost: rosson.k2.dev\r\nConnection: close\r\n\r\n")
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
            text.contains("200 OK") && text.contains("OK"),
            "expected the spliced 200 response, got:\n{text}"
        );

        subdomains::store(SubdomainMap::default());
        // `_home` (TempHome) restores $HOME and removes the tempdir on drop.
    }
}
