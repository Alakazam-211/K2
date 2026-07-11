//! K2 Connect **Pro multi-subdomain** routing map (PRD
//! `k2-connect-e2e-encryption.md` §7).
//!
//! A Pro user can register several nested subdomains under their personal
//! subdomain — `staging.<sub>.k2.dev`, `<git-sha>.<sub>.k2.dev`, … — each
//! pointing at a DIFFERENT internal endpoint on the same machine (e.g.
//! `localhost:3000`). Every nested host is routed by the relay's `*.<sub>.k2.dev`
//! wildcard to the user's ONE frpc tunnel, and the daemon's per-user wildcard
//! cert (E2E-1/E2E-3) already covers them — so creating a nested subdomain is
//! purely a *routing entry*: `<label> → target_endpoint`.
//!
//! ## What this module owns
//!
//! * [`SubdomainMap`] — the cached `label → target_endpoint` table plus the
//!   user's primary `subdomain` label, and the pure host-routing decision
//!   ([`SubdomainMap::route_for_host`]). O(1) per connection.
//! * A process-global [`arc_swap::ArcSwap`] cache the TLS listener reads on
//!   every connection ([`current`] / [`store`]), hot-swappable by the refresh
//!   loop with zero listener churn — same pattern as the cert hot-swap.
//! * [`fetch_map`] — the authenticated `GET <control-plane>/subdomains` call
//!   that the daemon's refresh loop uses to learn the account's subdomains
//!   over the SAME bearer-token channel the cert broker uses (the token in
//!   `~/.k2/tunnel.json`).
//!
//! ## How the daemon learns the map
//!
//! The control plane is the source of truth (Supabase-backed, §7). On the
//! lease cadence the daemon calls `GET /subdomains` with its tunnel bearer
//! token, parses `{ subdomains: [{label, target, ...}] }`, and stores the
//! result here. The TLS listener then routes inbound connections by Host.
//! Off by default: with E2E disabled the listener never even consults this.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use serde::Deserialize;

use super::config::SUBDOMAIN_HOST;

/// Default K2 Connect control-plane base (the deployed contract). The
/// `/subdomains` API lives here. Overridable via [`CONTROL_PLANE_BASE_ENV`]
/// for staging / integration tests — mirrors the cert broker's
/// `K2_CERT_BROKER_URL` override.
pub const DEFAULT_CONTROL_PLANE_BASE: &str = "https://connect.k2.dev";

/// Env var overriding [`DEFAULT_CONTROL_PLANE_BASE`].
pub const CONTROL_PLANE_BASE_ENV: &str = "K2_CONNECT_BASE";

/// HTTP timeout for a single `/subdomains` fetch. Short — a hung network
/// must never wedge the refresh loop past a tick; a missed refresh just
/// serves the previous (cached) map.
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

/// Resolve the control-plane base URL: [`CONTROL_PLANE_BASE_ENV`] if set
/// (non-blank), else [`DEFAULT_CONTROL_PLANE_BASE`]. A trailing slash is
/// trimmed so endpoint joins are clean.
pub fn control_plane_base() -> String {
    let base = match std::env::var(CONTROL_PLANE_BASE_ENV) {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => DEFAULT_CONTROL_PLANE_BASE.to_string(),
    };
    base.trim_end_matches('/').to_string()
}

/// Where an inbound TLS connection should be routed after the daemon
/// terminates TLS, decided purely from its Host/SNI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// Route to the daemon itself (the existing behaviour: splice the
    /// decrypted stream to the local HTTP dispatcher). This is the primary
    /// `<sub>.k2.dev`, or any host we don't recognise as a *configured*
    /// nested label (fail-safe: never strand the primary tunnel).
    Daemon,
    /// Proxy the decrypted stream to this internal endpoint (e.g.
    /// `localhost:3000`) — a configured nested Pro subdomain.
    Internal(String),
    /// A nested label under the user's subdomain that has NO configured
    /// target. Reject cleanly (404 / clean close) rather than leaking to the
    /// daemon — an unprovisioned `<label>.<sub>.k2.dev` is not a daemon route.
    UnknownNested,
}

/// One subdomain row from the control plane's `GET /subdomains` response.
/// Only the fields the daemon's routing needs are pulled; the rest
/// (cert_status, status, …) are ignored here — the control plane owns them.
#[derive(Debug, Clone, Deserialize)]
struct SubdomainRow {
    label: String,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    primary: bool,
}

#[derive(Debug, Deserialize)]
struct SubdomainsResponse {
    #[serde(default)]
    subdomains: Vec<SubdomainRow>,
}

/// The cached routing table: the user's primary subdomain label plus the
/// `nested-label → internal target_endpoint` map. Cheap to clone (it's held
/// behind an `Arc`), looked up O(1) per connection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubdomainMap {
    /// The user's primary subdomain label (e.g. `rosson`). Hosts of the
    /// form `<sub>.k2.dev` route to the daemon. Empty when unknown (then we
    /// fail safe: only EXACT nested-label matches proxy; everything else →
    /// daemon).
    pub primary: String,
    /// `nested label → internal target endpoint`. Only NON-primary,
    /// targeted rows live here.
    pub targets: HashMap<String, String>,
}

impl SubdomainMap {
    /// Build a map from the primary label + the control-plane rows. The
    /// primary row (and any row without a target) is excluded from
    /// `targets`; the primary label is recorded separately.
    fn from_rows(primary: &str, rows: Vec<SubdomainRow>) -> Self {
        let primary = primary.trim().to_ascii_lowercase();
        let mut targets = HashMap::new();
        for row in rows {
            let label = row.label.trim().to_ascii_lowercase();
            if label.is_empty() || row.primary || label == primary {
                continue; // primary routes to the daemon, never proxied.
            }
            if let Some(t) = row.target.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                targets.insert(label, t.to_string());
            }
        }
        Self { primary, targets }
    }

    /// Decide where a connection for `host` goes. `host` is the SNI/Host the
    /// daemon saw after TLS termination (case-insensitive; any `:port`
    /// suffix is ignored). Pure + O(1) — safe to call on the accept hot path.
    ///
    /// Rules (PRD §7):
    ///   * `<primary>.k2.dev`            → [`Route::Daemon`] (the primary tunnel)
    ///   * `<label>.<primary>.k2.dev`    → [`Route::Internal`] if `label` has a
    ///                                     configured target, else
    ///                                     [`Route::UnknownNested`]
    ///   * anything else (incl. an empty host, or a host outside this user's
    ///     subdomain space) → [`Route::Daemon`] (fail safe — never strand the
    ///     daemon's own traffic over a routing-map miss)
    pub fn route_for_host(&self, host: &str) -> Route {
        let host = host.trim().to_ascii_lowercase();
        // Drop any port suffix (`host:443`).
        let host = host.split(':').next().unwrap_or("").trim_end_matches('.');
        if host.is_empty() {
            return Route::Daemon;
        }

        // `<primary>.k2.dev` is the primary tunnel → daemon.
        if self.primary.is_empty() {
            // No known primary: only an EXACT configured nested target can
            // match (defensive — without a primary we can't classify nesting),
            // otherwise daemon.
            return self
                .targets
                .get(host)
                .map(|t| Route::Internal(t.clone()))
                .unwrap_or(Route::Daemon);
        }

        let primary_host = format!("{}.{}", self.primary, SUBDOMAIN_HOST);
        if host == primary_host {
            return Route::Daemon;
        }

        // Nested form: `<label>.<primary>.k2.dev`. Strip the
        // `.<primary>.k2.dev` suffix to get the label.
        let nested_suffix = format!(".{primary_host}");
        if let Some(label) = host.strip_suffix(&nested_suffix) {
            // A multi-segment label (`a.b.<primary>.k2.dev`) still resolves by
            // its full leading segment string; we look that whole thing up.
            if label.is_empty() {
                return Route::Daemon;
            }
            return match self.targets.get(label) {
                Some(t) => Route::Internal(t.clone()),
                None => Route::UnknownNested,
            };
        }

        // Outside this user's subdomain space (shouldn't happen given the
        // relay only routes `*.<sub>.k2.dev` here, but fail safe to daemon).
        Route::Daemon
    }
}

// ── Process-global cache (hot-swappable, like the cert ServerConfig) ──────

fn cache() -> &'static ArcSwap<SubdomainMap> {
    use std::sync::OnceLock;
    static CACHE: OnceLock<ArcSwap<SubdomainMap>> = OnceLock::new();
    CACHE.get_or_init(|| ArcSwap::from_pointee(SubdomainMap::default()))
}

/// The current cached routing map (cheap `Arc` clone). The TLS listener calls
/// this once per connection. Defaults to an empty map (everything → daemon)
/// until the first successful refresh — so before the daemon has learned the
/// account's subdomains, behaviour is exactly today's (daemon-only).
pub fn current() -> Arc<SubdomainMap> {
    cache().load_full()
}

/// Hot-swap the cached map. Called by the refresh loop after a successful
/// fetch; a concurrent [`current`] reader keeps serving the old map until the
/// store lands, so there's no listener churn.
///
/// **Change broadcast (URLs & Ports drawer):** when the landed map DIFFERS
/// from the previously-cached one, fire
/// [`HookEvent::TunnelSubdomainsChanged`](crate::agent_hooks::HookEvent) so
/// the daemon can mirror a `tunnel_subdomains_changed` frame onto its
/// session-events bus. Doing the compare HERE (the single store chokepoint)
/// means every caller — the connector's refresh loop today, anything else
/// tomorrow — gets change detection for free, and an UNCHANGED refresh tick
/// (the steady state, every lease interval) emits nothing.
pub fn store(map: SubdomainMap) {
    if swap_and_changed(map) {
        crate::agent_hooks::emit(
            crate::agent_hooks::HookEvent::TunnelSubdomainsChanged,
            serde_json::Value::Null,
        );
    }
}

/// Swap the cache to `map`; report whether the newly-stored map differs
/// from what was cached before. Split from [`store`] so the compare logic
/// is testable without touching the process-global hook sink.
fn swap_and_changed(map: SubdomainMap) -> bool {
    let changed = **cache().load() != map;
    cache().store(Arc::new(map));
    changed
}

// ── Control-plane fetch (the daemon's "learn the map" call) ───────────────

/// Fetch the account's subdomains from the control plane and return the
/// routing map. `primary` is the user's primary subdomain label (from the
/// tunnel config) used to classify the primary vs. nested rows; `token` is
/// the tunnel bearer token (same one the cert broker sends).
///
/// Fail-loud on transport/HTTP/parse errors so the caller can log + retry on
/// the next tick (it keeps serving the cached map meanwhile — never silently
/// drops nested routing).
pub fn fetch_map(primary: &str, token: &str) -> Result<SubdomainMap, String> {
    if token.trim().is_empty() {
        return Err("no tunnel bearer token (cannot fetch subdomains)".to_string());
    }
    let url = format!("{}/subdomains", control_plane_base());
    let client = reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| format!("http client build failed: {e}"))?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token.trim()))
        .send()
        .map_err(|e| format!("GET {url}: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .map_err(|e| format!("read /subdomains response: {e}"))?;
    if !status.is_success() {
        return Err(format!("/subdomains rejected (HTTP {status}): {}", truncate(&text, 200)));
    }
    let parsed: SubdomainsResponse = serde_json::from_str(&text)
        .map_err(|e| format!("parse /subdomains response: {e} (body: {})", truncate(&text, 200)))?;
    Ok(SubdomainMap::from_rows(primary, parsed.subdomains))
}

/// Refresh the global cache once from the control plane. Returns the number
/// of nested targets now cached on success. A failure is returned (caller
/// logs + retries); the previous cache is left intact.
pub fn refresh_once(primary: &str, token: &str) -> Result<usize, String> {
    let map = fetch_map(primary, token)?;
    let n = map.targets.len();
    store(map);
    Ok(n)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(label: &str, target: Option<&str>, primary: bool) -> SubdomainRow {
        SubdomainRow {
            label: label.to_string(),
            target: target.map(str::to_string),
            primary,
        }
    }

    #[test]
    fn primary_host_routes_to_daemon() {
        let map = SubdomainMap::from_rows(
            "rosson",
            vec![
                row("rosson", None, true),
                row("staging", Some("localhost:3000"), false),
            ],
        );
        assert_eq!(map.route_for_host("rosson.k2.dev"), Route::Daemon);
        // Case + trailing dot + port are all normalised.
        assert_eq!(map.route_for_host("ROSSON.k2.dev:443"), Route::Daemon);
        assert_eq!(map.route_for_host("rosson.k2.dev."), Route::Daemon);
    }

    #[test]
    fn configured_nested_label_routes_to_its_port() {
        let map = SubdomainMap::from_rows(
            "rosson",
            vec![
                row("rosson", None, true),
                row("staging", Some("localhost:3000"), false),
                row("preview", Some("127.0.0.1:8080"), false),
            ],
        );
        assert_eq!(
            map.route_for_host("staging.rosson.k2.dev"),
            Route::Internal("localhost:3000".to_string())
        );
        assert_eq!(
            map.route_for_host("preview.rosson.k2.dev"),
            Route::Internal("127.0.0.1:8080".to_string())
        );
    }

    #[test]
    fn unknown_nested_label_is_rejected() {
        let map = SubdomainMap::from_rows(
            "rosson",
            vec![
                row("rosson", None, true),
                row("staging", Some("localhost:3000"), false),
            ],
        );
        // A nested label with no configured target → rejected, NOT daemon
        // (an unprovisioned nested host is not the primary tunnel).
        assert_eq!(
            map.route_for_host("ghost.rosson.k2.dev"),
            Route::UnknownNested
        );
    }

    #[test]
    fn primary_row_and_targetless_rows_excluded_from_targets() {
        let map = SubdomainMap::from_rows(
            "rosson",
            vec![
                row("rosson", Some("daemon"), true), // primary, even if it has a target
                row("nolist", None, false),          // no target → excluded
                row("staging", Some("localhost:3000"), false),
            ],
        );
        assert_eq!(map.targets.len(), 1, "only the one targeted nested row");
        assert!(map.targets.contains_key("staging"));
        assert!(!map.targets.contains_key("rosson"));
        assert!(!map.targets.contains_key("nolist"));
        // A targetless nested host falls through to UnknownNested (it IS a
        // nested host, just unprovisioned).
        assert_eq!(map.route_for_host("nolist.rosson.k2.dev"), Route::UnknownNested);
    }

    #[test]
    fn empty_or_foreign_host_routes_to_daemon_fail_safe() {
        let map = SubdomainMap::from_rows("rosson", vec![row("staging", Some("localhost:3000"), false)]);
        assert_eq!(map.route_for_host(""), Route::Daemon);
        assert_eq!(map.route_for_host("   "), Route::Daemon);
        // A host outside the user's subdomain space → daemon (never strand
        // the primary tunnel over a routing miss).
        assert_eq!(map.route_for_host("someone-else.k2.dev"), Route::Daemon);
        assert_eq!(map.route_for_host("evil.example.com"), Route::Daemon);
    }

    #[test]
    fn no_known_primary_only_exact_target_matches() {
        // Defensive path: if we somehow have targets but no primary label,
        // only an EXACT host match in `targets` proxies; everything else is
        // daemon (we can't classify nesting without the primary).
        let mut targets = HashMap::new();
        targets.insert("staging.rosson.k2.dev".to_string(), "localhost:3000".to_string());
        let map = SubdomainMap {
            primary: String::new(),
            targets,
        };
        assert_eq!(
            map.route_for_host("staging.rosson.k2.dev"),
            Route::Internal("localhost:3000".to_string())
        );
        assert_eq!(map.route_for_host("rosson.k2.dev"), Route::Daemon);
    }

    #[test]
    fn from_rows_lowercases_labels_and_primary() {
        let map = SubdomainMap::from_rows(
            "Rosson",
            vec![row("STAGING", Some("localhost:3000"), false)],
        );
        assert_eq!(map.primary, "rosson");
        // Lookup is case-insensitive because both store + query lowercase.
        assert_eq!(
            map.route_for_host("Staging.Rosson.K2.dev"),
            Route::Internal("localhost:3000".to_string())
        );
    }

    #[test]
    fn parse_control_plane_response_shape() {
        let body = r#"{"subdomains":[
            {"label":"rosson","host":"rosson.k2.dev","target":"daemon","status":"active","primary":true,"effective":"live"},
            {"label":"staging","host":"staging.rosson.k2.dev","target":"localhost:3000","status":"active","primary":false,"effective":"live"}
        ]}"#;
        let parsed: SubdomainsResponse = serde_json::from_str(body).expect("parse");
        let map = SubdomainMap::from_rows("rosson", parsed.subdomains);
        assert_eq!(map.targets.len(), 1);
        assert_eq!(
            map.route_for_host("staging.rosson.k2.dev"),
            Route::Internal("localhost:3000".to_string())
        );
        assert_eq!(map.route_for_host("rosson.k2.dev"), Route::Daemon);
    }

    #[test]
    fn control_plane_base_default_and_override() {
        let _g = crate::themes::HOME_LOCK.lock();
        let prev = std::env::var_os(CONTROL_PLANE_BASE_ENV);
        std::env::remove_var(CONTROL_PLANE_BASE_ENV);
        assert_eq!(control_plane_base(), DEFAULT_CONTROL_PLANE_BASE);
        std::env::set_var(CONTROL_PLANE_BASE_ENV, "http://127.0.0.1:9999/");
        // Trailing slash trimmed.
        assert_eq!(control_plane_base(), "http://127.0.0.1:9999");
        std::env::set_var(CONTROL_PLANE_BASE_ENV, "   ");
        assert_eq!(control_plane_base(), DEFAULT_CONTROL_PLANE_BASE);
        match prev {
            Some(p) => std::env::set_var(CONTROL_PLANE_BASE_ENV, p),
            None => std::env::remove_var(CONTROL_PLANE_BASE_ENV),
        }
    }

    #[test]
    fn fetch_map_empty_token_is_error() {
        let err = fetch_map("rosson", "  ").expect_err("empty token must error");
        assert!(err.contains("no tunnel bearer token"), "got: {err}");
    }

    #[test]
    fn fetch_map_hits_subdomains_endpoint_with_bearer() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::mpsc;

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mock");
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                let _ = tx.send(req);
                let body = r#"{"subdomains":[{"label":"staging","target":"localhost:3000","primary":false}]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes());
            }
        });

        let _g = crate::themes::HOME_LOCK.lock();
        let prev = std::env::var_os(CONTROL_PLANE_BASE_ENV);
        std::env::set_var(CONTROL_PLANE_BASE_ENV, format!("http://127.0.0.1:{port}"));

        let map = fetch_map("rosson", "tok_abc").expect("fetch must succeed");

        match prev {
            Some(p) => std::env::set_var(CONTROL_PLANE_BASE_ENV, p),
            None => std::env::remove_var(CONTROL_PLANE_BASE_ENV),
        }

        let req = rx.recv_timeout(Duration::from_secs(5)).expect("server saw a request");
        assert!(req.starts_with("GET /subdomains "), "must GET /subdomains:\n{req}");
        // reqwest lowercases header names on the wire — match case-insensitively.
        assert!(
            req.to_ascii_lowercase().contains("authorization: bearer tok_abc"),
            "must send bearer token:\n{req}"
        );
        assert_eq!(
            map.route_for_host("staging.rosson.k2.dev"),
            Route::Internal("localhost:3000".to_string())
        );
    }

    #[test]
    fn fetch_map_non_2xx_is_error() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mock");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf);
                let body = r#"{"error":"pro_required"}"#;
                let resp = format!(
                    "HTTP/1.1 403 Forbidden\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes());
            }
        });

        let _g = crate::themes::HOME_LOCK.lock();
        let prev = std::env::var_os(CONTROL_PLANE_BASE_ENV);
        std::env::set_var(CONTROL_PLANE_BASE_ENV, format!("http://127.0.0.1:{port}"));
        let err = fetch_map("rosson", "tok").expect_err("403 must be an error");
        match prev {
            Some(p) => std::env::set_var(CONTROL_PLANE_BASE_ENV, p),
            None => std::env::remove_var(CONTROL_PLANE_BASE_ENV),
        }
        assert!(err.contains("HTTP 403"), "got: {err}");
    }

    #[test]
    fn global_cache_defaults_empty_and_stores() {
        // Default cache routes everything to the daemon (no nested targets).
        // NOTE: process-global; this test only asserts the store/load round
        // trip with a sentinel it sets itself. Keep it the ONLY test that
        // touches the global cache so the change-detection asserts below
        // can't race a parallel store.
        let map = SubdomainMap::from_rows("rosson", vec![row("staging", Some("localhost:3000"), false)]);
        // Landing a DIFFERENT map reports changed (→ store() broadcasts)…
        assert!(
            swap_and_changed(map.clone()),
            "sentinel map must differ from the previously-cached map"
        );
        // …and re-landing the IDENTICAL map does not — the steady-state
        // refresh tick (same map every lease interval) must stay silent.
        assert!(
            !swap_and_changed(map.clone()),
            "re-storing an identical map must NOT report a change"
        );
        let loaded = current();
        assert_eq!(loaded.route_for_host("staging.rosson.k2.dev"), Route::Internal("localhost:3000".to_string()));
        // Reset to empty so we don't bleed into other tests in this binary
        // (the reset itself is a change — the map goes sentinel → empty).
        assert!(swap_and_changed(SubdomainMap::default()));
        assert_eq!(current().route_for_host("staging.rosson.k2.dev"), Route::Daemon);
    }
}
