//! K2 Connect tunnel connector (CLIENT / open side).
//!
//! This is the **open-core, MIT** half: the daemon-side machinery that
//! exposes the local K2SO daemon to the internet at
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
//!   * [`config`]    — `~/.k2so/tunnel.json` (the secret token lives here).
//!   * [`render`]    — frpc v0.61 TOML renderer.
//!   * [`connector`] — spawn / supervise / stop the `frpc` child, and
//!                     drive the daemon-owned subdomain lease renewal
//!                     while the tunnel is up.
//!   * [`lease`]     — subdomain claim/lease keepalive (K2SO #674): the
//!                     daemon re-POSTs the `claim_subdomain` RPC on its own
//!                     timer so the lease never lapses with the UI closed
//!                     or the daemon headless.
//!
//! Public facade ([`start_tunnel`] / [`stop_tunnel`] / [`tunnel_status`])
//! is what the daemon's `/cli/tunnel/*` routes call.

use serde::{Deserialize, Serialize};

pub mod config;
pub mod connector;
pub mod lease;
pub mod render;

pub use config::TunnelConfig;
pub use connector::{FrpcBinary, TunnelStatus};

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
/// spawning anything. Handy for diagnostics / `--dry-run`.
pub fn render_config(local_port: u16) -> Result<String, String> {
    let cfg = config::load()?;
    Ok(render::render_frpc_toml(&cfg, local_port))
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Shared test scaffolding. Tunnel tests touch `$HOME` (config +
    //! frpc.toml + log all live under `~/.k2so/`), so they must
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
