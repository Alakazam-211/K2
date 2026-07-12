//! K2 Connect tunnel connector (CLIENT / source-available side).
//!
//! This is the **open-core** half: the daemon-side machinery that
//! exposes the local K2 daemon to the internet at
//! `https://<user>.k2.dev` by running an `frpc` client that dials the
//! hosted (proprietary) K2 Connect frps server.
//!
//! Pipeline: `frpc (this machine) → frps (Hetzner) → Caddy (*.k2.dev TLS)
//! → https://{user}.k2.dev`.
//!
//! The control plane authorizes each frpc Login by validating the K2SO
//! bearer token carried in the login metas, and *forces* the proxy
//! subdomain to the token's canonical `{user}` namespace. So the client
//! supplies `{ token, requested-subdomain, localPort }`; the server
//! canonicalizes. See the proprietary `k2-connect` repo for the server
//! contract.
//!
//! Modules:
//!   * [`config`]    — `~/.k2/tunnel.json` (the secret token lives here).
//!   * [`render`]    — frpc v0.61 TOML renderer.
//!   * [`connector`] — spawn / supervise / stop the `frpc` child, and
//!                     drive the daemon-owned subdomain lease renewal
//!                     while the tunnel is up.
//!   * [`failover`]  — pure multi-relay failover policy: which relay of
//!                     the ordered fallback list frpc should dial now
//!                     (frpc itself can't fail over — one `serverAddr`).
//!   * [`lease`]     — subdomain claim/lease keepalive (K2SO #674): the
//!                     daemon re-POSTs the `claim_subdomain` RPC on its own
//!                     timer so the lease never lapses with the UI closed
//!                     or the daemon headless.
//!   * [`tls`]       — E2E TLS material: ECDSA key, CSR, rustls ServerConfig
//!                     (PRD `k2-connect-e2e-encryption.md` §4/§6).
//!   * [`cert_broker`] — E2E cert-broker client: POST the CSR to
//!                     `cert.k2.dev/cert`, install the issued chain, and
//!                     report renewal timing (PRD §5/§6).
//!
//! Public facade ([`start_tunnel`] / [`stop_tunnel`] / [`tunnel_status`])
//! is what the daemon's `/cli/tunnel/*` routes call.

use serde::{Deserialize, Serialize};

pub mod cert_broker;
pub mod config;
pub mod connector;
pub mod failover;
pub mod lease;
pub mod render;
pub mod subdomains;
pub mod tls;

pub use config::{e2e_enabled, RelayEndpoint, TunnelConfig};
pub use connector::{FrpcBinary, TunnelStatus};
pub use failover::{RelaySelector, RelaySwitch};

/// Redacted view of the tunnel config for the UI. NEVER carries the
/// secret token — only `tokenSet`. Field names are camelCase to match
/// the renderer's `TunnelConfigView` interface.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelConfigView {
    pub server_addr: String,
    pub server_port: u16,
    pub subdomain: String,
    pub token_set: bool,
    pub public_url: Option<String>,
    /// Re-launch this tunnel on daemon boot (camelCase `autoStart`).
    pub auto_start: bool,
    /// End-to-end encryption preference as STORED in the config (camelCase
    /// `e2e`). ON by default (0.40.6+); a user opts out by setting it false.
    /// This is the persisted field — not the env-override-resolved effective
    /// state; see `e2eEffective` for what the daemon will actually do.
    pub e2e: bool,
    /// The EFFECTIVE E2E state the daemon will use, after applying the
    /// `K2_E2E` env override on top of the stored field (camelCase
    /// `e2eEffective`). The renderer should render the toggle from `e2e`
    /// but can surface this to explain when an env var is overriding the
    /// stored preference.
    pub e2e_effective: bool,
}

impl From<&TunnelConfig> for TunnelConfigView {
    fn from(c: &TunnelConfig) -> Self {
        Self {
            server_addr: c.server_addr.clone(),
            server_port: c.server_port,
            subdomain: c.subdomain.clone(),
            token_set: !c.token.trim().is_empty(),
            public_url: c.public_url(),
            auto_start: c.auto_start,
            e2e: c.e2e,
            e2e_effective: config::e2e_enabled(c),
        }
    }
}

/// Partial config update from the UI. Absent fields leave the stored
/// value untouched; a blank `token` is ignored so re-saving the other
/// fields can't wipe the secret.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelConfigUpdate {
    pub server_addr: Option<String>,
    pub server_port: Option<u16>,
    pub subdomain: Option<String>,
    pub token: Option<String>,
    pub auto_start: Option<bool>,
    /// End-to-end encryption opt-out (0.40.6+). Absent → leave the stored
    /// value untouched (default-on). `Some(false)` is the explicit user
    /// opt-out (legacy terminating path); `Some(true)` re-enables E2E.
    pub e2e: Option<bool>,
    /// Stable per-install device id for the subdomain lease (K2SO #674).
    /// The renderer persists its claim identity here so the daemon renews
    /// under the same device.
    pub device_id: Option<String>,
    /// Cosmetic device label that accompanies `device_id`.
    pub device_label: Option<String>,
}

/// Read the stored tunnel config as a redacted view (token stays in the
/// daemon — only `tokenSet` crosses the wire).
pub fn get_config_view() -> Result<TunnelConfigView, String> {
    Ok((&config::load()?).into())
}

/// Apply a partial config update, persist it, and return the redacted
/// view. A blank/absent token is ignored so the secret survives re-saves.
pub fn set_config(upd: TunnelConfigUpdate) -> Result<TunnelConfigView, String> {
    let cfg = config::update(|c| {
        if let Some(a) = upd.server_addr {
            if !a.trim().is_empty() {
                c.server_addr = a.trim().to_string();
            }
        }
        if let Some(p) = upd.server_port {
            if p > 0 {
                c.server_port = p;
            }
        }
        if let Some(s) = upd.subdomain {
            c.subdomain = s.trim().to_string();
        }
        if let Some(t) = upd.token {
            if !t.trim().is_empty() {
                c.token = t.trim().to_string();
            }
        }
        if let Some(a) = upd.auto_start {
            c.auto_start = a;
        }
        // E2E opt-out (0.40.6+). An explicit value (true to keep/restore the
        // default-on E2E path, false to opt out to the legacy terminating
        // path) is persisted; absent leaves the stored preference untouched.
        if let Some(e) = upd.e2e {
            c.e2e = e;
        }
        // Device identity for the lease (K2SO #674). A blank value is
        // ignored so re-saving other fields can't wipe a stored id; an
        // explicit non-blank value updates it.
        if let Some(d) = upd.device_id {
            if !d.trim().is_empty() {
                c.device_id = Some(d.trim().to_string());
            }
        }
        if let Some(l) = upd.device_label {
            let l = l.trim();
            c.device_label = if l.is_empty() { None } else { Some(l.to_string()) };
        }
    })?;
    Ok((&cfg).into())
}

/// Start the tunnel using the stored config (auto-locating `frpc`).
///
/// * `subdomain` — optional override for the requested subdomain
///   (persisted to the config when present).
/// * `daemon_port` — the live daemon HTTP port to expose when the config
///   doesn't pin a `local_port`.
pub fn start_tunnel(
    subdomain: Option<String>,
    daemon_port: u16,
) -> Result<TunnelStatus, String> {
    connector::start(subdomain, daemon_port, &FrpcBinary::Auto)
}

/// Stop the tunnel (kills the supervised `frpc` child; no restart).
pub fn stop_tunnel() -> Result<(), String> {
    connector::stop()
}

/// Current tunnel status (running? + predicted public URL).
pub fn tunnel_status() -> TunnelStatus {
    connector::status()
}

/// Render the frpc TOML for the stored config + given local port, without
/// spawning anything. Handy for diagnostics / `--dry-run`. Reflects the
/// current E2E flag ([`config::e2e_enabled`]) so the dry-run output matches
/// what `start_tunnel` would actually write.
pub fn render_config(local_port: u16) -> Result<String, String> {
    let cfg = config::load()?;
    let e2e = config::e2e_enabled(&cfg);
    Ok(render::render_frpc_toml(&cfg, local_port, e2e))
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Shared test scaffolding. Tunnel tests touch `$HOME` (config +
    //! frpc.toml + log all live under `~/.k2/`), so they must
    //! serialize and redirect HOME to a tempdir. We reuse the crate-wide
    //! `themes::HOME_LOCK` so we never race the other HOME-mutating test
    //! suites (app_settings, themes, companion).

    use crate::themes::HOME_LOCK;

    /// Run `f` with `$HOME` pointed at a fresh tempdir, under the global
    /// HOME lock, and clean up afterward. Also clears any prior tunnel
    /// connector singleton state so start/stop tests don't bleed.
    pub fn with_temp_home<F: FnOnce()>(f: F) {
        // parking_lot::Mutex — `lock()` returns the guard directly.
        let _g = HOME_LOCK.lock();
        let prev = std::env::var_os("HOME");
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp = std::env::temp_dir().join(format!("k2so-tunnel-test-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp HOME");
        std::env::set_var("HOME", &tmp);

        // Ensure a clean connector singleton for this test.
        let _ = super::connector::stop();

        f();

        // Best-effort connector teardown + HOME restore.
        let _ = super::connector::stop();
        match prev {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
