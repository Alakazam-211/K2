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
//!   * [`watchdog`]  — pure mid-session relay-death detection: frpc never
//!                     exits when an ESTABLISHED session's relay dies (it
//!                     reconnect-retries internally), so the connector
//!                     watches its log lines and kills the stuck child to
//!                     hand the failure to the exit-based failover path.
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
pub mod watchdog;

pub use config::{e2e_enabled, RelayEndpoint, TunnelConfig};
pub use connector::{FrpcBinary, TunnelStatus};
pub use failover::{RelaySelector, RelaySwitch};
pub use watchdog::{frpc_line_signals_disconnect, frpc_line_signals_recovery, DisconnectTracker};

/// Redacted view of the tunnel config for the UI. NEVER carries the
/// secret token — only `tokenSet`. Field names are camelCase to match
/// the renderer's `TunnelConfigView` interface.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelConfigView {
    pub server_addr: String,
    pub server_port: u16,
    /// Ordered relay fallback list as STORED (index 0 = preferred). Hosts
    /// and ports only — nothing secret — so the redacted view carries it
    /// verbatim. Empty (every legacy single-endpoint config) is omitted
    /// from the JSON, mirroring the on-disk shape, so a pre-failover
    /// client sees a byte-identical view.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub relays: Vec<RelayEndpoint>,
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
            relays: c.relays.clone(),
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
    /// Ordered relay fallback list (multi-relay edge resilience). Absent →
    /// the stored list is untouched, so a legacy client that never sends
    /// the field changes NOTHING. Present replaces the list wholesale; an
    /// explicit empty list is the "clear" verb — back to the legacy single
    /// `server_addr`/`server_port` endpoint (via `relay_list()`). Like
    /// every other field here this only PERSISTS config: a currently-
    /// connected tunnel is not disturbed, the new list applies when the
    /// tunnel next (re)starts.
    pub relays: Option<Vec<RelayEndpoint>>,
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
        // Relay fallback list. Absent → untouched (legacy clients never
        // send it). Present replaces wholesale; an explicit `[]` clears the
        // list, reverting to the legacy single-endpoint pair. Persist-only:
        // a running tunnel keeps the relays it started with until the next
        // (re)start reads the config — exactly like server_addr above.
        if let Some(r) = upd.relays {
            c.relays = r;
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
    // Dial the preferred relay (index 0), exactly as `start_tunnel` does —
    // for a legacy single-endpoint config this IS server_addr/server_port.
    let relays = cfg.relay_list();
    Ok(render::render_frpc_toml_for_relay(&cfg, &relays[0], local_port, e2e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_support::with_temp_home;

    fn ep(host: &str, port: u16) -> RelayEndpoint {
        RelayEndpoint {
            host: host.to_string(),
            port,
        }
    }

    #[test]
    fn update_json_without_relays_field_deserializes_to_none() {
        // The legacy-compat contract at the wire: a pre-failover client
        // POSTing its usual body must parse with relays = None, which
        // set_config maps to "change NOTHING" about the stored list.
        let upd: TunnelConfigUpdate =
            serde_json::from_str(r#"{"subdomain":"rosson","token":"tok"}"#)
                .expect("parse legacy update body");
        assert!(upd.relays.is_none(), "absent relays must deserialize None");
    }

    #[test]
    fn update_json_with_relays_parses_ordered_with_port_default() {
        // The new wire shape mirrors tunnel.json: ordered array, index 0 =
        // primary, a host-only entry gets the default frps port.
        let upd: TunnelConfigUpdate = serde_json::from_str(
            r#"{"relays":[{"host":"10.0.0.1","port":7001},{"host":"10.0.0.2"}]}"#,
        )
        .expect("parse relays update body");
        assert_eq!(
            upd.relays,
            Some(vec![
                ep("10.0.0.1", 7001),
                ep("10.0.0.2", config::DEFAULT_SERVER_PORT),
            ]),
            "relays must parse in order with the port defaulting"
        );
    }

    #[test]
    fn set_config_with_relays_persists_and_view_round_trips() {
        with_temp_home(|| {
            let list = vec![ep("10.0.0.1", 7000), ep("10.0.0.2", 7000)];
            let view = set_config(TunnelConfigUpdate {
                relays: Some(list.clone()),
                subdomain: Some("rosson".to_string()),
                token: Some("tok".to_string()),
                ..Default::default()
            })
            .expect("set_config with relays");
            assert_eq!(view.relays, list, "view must echo the new list back");
            // ...and the list must actually be ON DISK for the next start.
            let stored = config::load().expect("load");
            assert_eq!(stored.relays, list, "relays must persist via the update path");
            assert_eq!(stored.relay_list(), list, "list is authoritative when non-empty");
        });
    }

    #[test]
    fn set_config_without_relays_leaves_stored_list_untouched() {
        with_temp_home(|| {
            // Seed a multi-relay config, then apply a relays-LESS update
            // (what every legacy client sends). The stored list must
            // survive — absent field changes NOTHING.
            let list = vec![ep("10.0.0.1", 7000), ep("10.0.0.2", 7000)];
            config::save(&TunnelConfig {
                relays: list.clone(),
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                ..Default::default()
            })
            .expect("seed multi-relay config");
            let view = set_config(TunnelConfigUpdate {
                subdomain: Some("rosson2".to_string()),
                ..Default::default()
            })
            .expect("relays-less update");
            assert_eq!(view.subdomain, "rosson2", "the update itself applied");
            assert_eq!(
                config::load().expect("load").relays,
                list,
                "absent relays field must not touch the stored list"
            );
        });
    }

    #[test]
    fn set_config_empty_relays_clears_back_to_legacy_endpoint() {
        with_temp_home(|| {
            config::save(&TunnelConfig {
                server_addr: "9.9.9.9".to_string(),
                server_port: 7009,
                relays: vec![ep("10.0.0.1", 7000), ep("10.0.0.2", 7000)],
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                ..Default::default()
            })
            .expect("seed multi-relay config");
            // The explicit "clear" verb: `relays: []` empties the list, so
            // relay_list() folds back to the legacy single-endpoint pair.
            set_config(TunnelConfigUpdate {
                relays: Some(Vec::new()),
                ..Default::default()
            })
            .expect("clear relays");
            let stored = config::load().expect("load");
            assert!(stored.relays.is_empty(), "explicit [] must clear the list");
            assert_eq!(
                stored.relay_list(),
                vec![ep("9.9.9.9", 7009)],
                "cleared config must fold back to the legacy endpoint"
            );
        });
    }

    #[test]
    fn legacy_view_json_omits_relays_key() {
        with_temp_home(|| {
            // A legacy (relays-empty) config's redacted view must serialize
            // WITHOUT a relays key — a pre-failover client sees the exact
            // JSON shape it always did.
            config::save(&TunnelConfig {
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                ..Default::default()
            })
            .expect("seed legacy config");
            let view = get_config_view().expect("view");
            let json = serde_json::to_string(&view).expect("serialize view");
            assert!(
                !json.contains("relays"),
                "empty relay list must be skipped in the view JSON\n{json}"
            );
        });
    }
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
