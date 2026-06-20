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

use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use rustls::ServerConfig;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

use k2_core::log_debug;

/// How often the cert-renewal loop wakes to check the installed cert's
/// `notAfter`. 12 h is far finer than the ~30-day renewal window, so a cert
/// is always re-issued well before expiry even across a daemon restart, yet
/// the loop is near-idle (two cheap cert parses a day). Mirrors the
/// lease-renewal loop's "schedule lives in the daemon" pattern.
const CERT_CHECK_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);

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
        return Ok(None); // OFF — default; zero behaviour change.
    }

    // A subdomain is required to name the cert SANs. When E2E is flagged on
    // but no subdomain is configured yet, fail loud — a cert for an empty
    // host is useless and would mask a misconfigured tunnel.
    let subdomain = cfg.subdomain.trim().to_string();
    if subdomain.is_empty() {
        return Err(
            "K2_E2E is enabled but no subdomain is configured in ~/.k2/tunnel.json — \
             cannot provision a TLS cert (SANs need <sub>.k2.dev)"
                .to_string(),
        );
    }

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

    Ok(Some(https_port))
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
                // Snapshot the live config for THIS connection (cheap Arc
                // clone). A concurrent hot-swap only affects later accepts.
                let acceptor = TlsAcceptor::from(shared_config.load_full());
                tokio::spawn(async move {
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
async fn serve_one(
    tcp: TcpStream,
    acceptor: TlsAcceptor,
    http_port: u16,
) -> Result<(), String> {
    use k2_core::tunnel::subdomains::{self, Route};

    let mut tls = acceptor
        .accept(tcp)
        .await
        .map_err(|e| format!("TLS handshake failed: {e}"))?;

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
            let mut upstream = TcpStream::connect(("127.0.0.1", http_port))
                .await
                .map_err(|e| format!("connect to local HTTP listener :{http_port}: {e}"))?;
            tokio::io::copy_bidirectional(&mut tls, &mut upstream)
                .await
                .map(|_| ())
                .map_err(|e| format!("splice TLS↔HTTP: {e}"))
        }
        Route::Internal(target) => {
            // Configured nested subdomain → proxy the decrypted stream to the
            // user's chosen internal endpoint on this same machine.
            let mut upstream = TcpStream::connect(target.as_str()).await.map_err(|e| {
                format!("connect to internal endpoint {target} for SNI {sni}: {e}")
            })?;
            log_debug!("[daemon/e2e] routing {sni} → internal endpoint {target}");
            tokio::io::copy_bidirectional(&mut tls, &mut upstream)
                .await
                .map(|_| ())
                .map_err(|e| format!("splice TLS↔internal {target}: {e}"))
        }
        Route::UnknownNested => {
            // A nested label with no configured target. Reply 404 + close —
            // do NOT silently fall through to the daemon (an unprovisioned
            // nested host is not a daemon route).
            use tokio::io::AsyncWriteExt;
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

    /// With E2E OFF (no env, no config flag), `maybe_spawn` is a no-op and
    /// returns `None` — zero behaviour change vs today.
    #[tokio::test]
    async fn maybe_spawn_is_noop_when_e2e_off() {
        // Defensive: ensure the env flag isn't leaking in from the shell.
        let prev = std::env::var_os("K2_E2E");
        std::env::remove_var("K2_E2E");
        // No subdomain / fresh config → e2e_enabled() is false → None.
        let res = maybe_spawn(12345).await.expect("must not error when off");
        assert!(res.is_none(), "E2E off must yield no HTTPS listener");
        if let Some(p) = prev {
            std::env::set_var("K2_E2E", p);
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
}
