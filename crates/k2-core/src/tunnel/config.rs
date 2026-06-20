//! Tunnel connector configuration — the `frpc` dial-in parameters for
//! exposing this daemon at `https://<subdomain>.k2.dev` via the hosted
//! K2 Connect backbone.
//!
//! **Filesystem-first / daemon-first.** Settings live in a dedicated
//! `~/.k2/tunnel.json` file (NOT the shared `settings.json`) so the
//! bearer token — a *secret* — has its own 0600 file outside any git
//! tree and is never logged. The daemon is the sole reader/writer.
//!
//! The on-disk shape is intentionally small and stable:
//! ```json
//! {
//!   "server_addr": "178.156.232.105",
//!   "server_port": 7000,
//!   "token": "<k2so-bearer>",
//!   "subdomain": "rosson",
//!   "local_port": 57839,
//!   "auto_start": false
//! }
//! ```
//!
//! `local_port` defaults to the running daemon's port at start time when
//! the caller doesn't pin one, so a fresh config only ever needs a token
//! (and optionally a subdomain).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Default frps control-plane address (the deployed Hetzner box). Host
/// only — the port is carried separately so the renderer can emit the
/// two fields frp's TOML schema expects.
pub const DEFAULT_SERVER_HOST: &str = "178.156.232.105";

/// Default frps `bindPort` (frpc dial-in). Matches `infra/frps.toml`.
pub const DEFAULT_SERVER_PORT: u16 = 7000;

/// The hosted subdomain zone. Every user lands under `<sub>.k2.dev`.
pub const SUBDOMAIN_HOST: &str = "k2.dev";

/// Connector configuration. Mirrors the contract the deployed
/// control-plane server expects: a bearer `token` (validated → user),
/// a requested `subdomain` (the server canonicalizes it to `{user}`),
/// and the `local_port` of the daemon to expose.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TunnelConfig {
    /// frps host (no port). Defaults to [`DEFAULT_SERVER_HOST`].
    #[serde(default = "default_server_host")]
    pub server_addr: String,

    /// frps `bindPort`. Defaults to [`DEFAULT_SERVER_PORT`].
    #[serde(default = "default_server_port")]
    pub server_port: u16,

    /// K2SO bearer token. Carried in the frpc login `metadatas.token`;
    /// the control plane resolves it to the owning user. **Secret** —
    /// never logged, stored in a 0600 file.
    #[serde(default)]
    pub token: String,

    /// Requested subdomain label. The server *forces* this to the
    /// user's canonical `{user}` namespace, so it's advisory; we send
    /// the user's intended label and let the server canonicalize.
    #[serde(default)]
    pub subdomain: String,

    /// Local port to expose — the daemon's HTTP port. When `None` the
    /// connector fills it with the live daemon port at start time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_port: Option<u16>,

    /// Opt-in: re-launch this tunnel automatically on daemon boot. When
    /// true AND the config [`is_connectable`](TunnelConfig::is_connectable),
    /// the daemon starts `frpc` for the saved subdomain once it's ready.
    /// Defaults false so a fresh / un-opted config never auto-dials.
    #[serde(default)]
    pub auto_start: bool,

    /// Stable per-install device id used for the subdomain claim/lease
    /// (K2SO #674). The renderer generates this once and persists it here
    /// via `/cli/tunnel/config` so the DAEMON renews the lease under the
    /// SAME identity the client claimed with — otherwise a daemon-side
    /// renewal would look like a different device taking over. `None` on a
    /// manual token-only config that never went through the account/claim
    /// flow (renewal is then skipped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,

    /// Human-readable device label (cosmetic; shown in the holder UI). Sent
    /// alongside `device_id` so the daemon's renewal carries the same label
    /// the client did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_label: Option<String>,

    /// **K2 Connect E2E encryption opt-in (Phase 0/1, PRD
    /// `k2-connect-e2e-encryption.md` §4 Option A).** When true the daemon
    /// terminates TLS itself (a rustls listener presents a cert for
    /// `<sub>.k2.dev`) and frpc forwards the *encrypted* stream
    /// (`type = "https"`) so the relay carries only ciphertext. When false
    /// (the default) behaviour is EXACTLY as today: frpc `type = "http"` and
    /// Caddy terminates TLS at the relay. The env var `K2_E2E=1` forces this
    /// on regardless of the stored value (see [`e2e_enabled`]); the field lets
    /// it be persisted per-install once the broker ships. Default false so no
    /// existing tunnel changes behaviour on upgrade.
    #[serde(default)]
    pub e2e: bool,
}

/// True when K2 Connect end-to-end encryption (Option A) is enabled for
/// this daemon. The single gate every E2E code path consults.
///
/// Enabled when EITHER the env var `K2_E2E` is set to a truthy value
/// (`1`/`true`/`yes`/`on`, case-insensitive) OR the passed config has
/// `e2e: true`. The env var is the spike/Phase-0 switch (no config write
/// needed, easy to flip in a smoke test); the config field is the
/// persisted form the broker task will set once issuance is wired. Default
/// (no env, `e2e:false`) is OFF → zero behaviour change.
pub fn e2e_enabled(cfg: &TunnelConfig) -> bool {
    if env_e2e_on() {
        return true;
    }
    cfg.e2e
}

/// Truthiness of the `K2_E2E` env var, isolated so it's unit-testable and
/// the parsing rule lives in exactly one place.
fn env_e2e_on() -> bool {
    match std::env::var("K2_E2E") {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

fn default_server_host() -> String {
    DEFAULT_SERVER_HOST.to_string()
}

fn default_server_port() -> u16 {
    DEFAULT_SERVER_PORT
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            server_addr: default_server_host(),
            server_port: default_server_port(),
            token: String::new(),
            subdomain: String::new(),
            local_port: None,
            auto_start: false,
            device_id: None,
            device_label: None,
            e2e: false,
        }
    }
}

impl TunnelConfig {
    /// True when the config carries enough to attempt a connection: a
    /// non-empty token and a server address. (`subdomain` empty is
    /// tolerated — the server will derive the canonical label from the
    /// token's user; a blank requested label simply means "give me my
    /// primary namespace".)
    pub fn is_connectable(&self) -> bool {
        !self.token.trim().is_empty() && !self.server_addr.trim().is_empty()
    }

    /// The public URL this config will resolve to *as requested* —
    /// `https://<subdomain>.k2.dev`. NOTE: the server canonicalizes the
    /// subdomain to the token's user, so if `subdomain` differs from the
    /// user's namespace the live URL will differ. Callers that need the
    /// authoritative URL should surface the server-confirmed label;
    /// this is the best-effort, pre-connect prediction.
    pub fn public_url(&self) -> Option<String> {
        let sub = self.subdomain.trim();
        if sub.is_empty() {
            return None;
        }
        Some(format!("https://{sub}.{SUBDOMAIN_HOST}"))
    }
}

/// Directory holding `~/.k2/tunnel.json`. Honors `$HOME` so tests can
/// redirect it to a tempdir.
fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
}

/// Path to the tunnel config file.
pub fn config_path() -> PathBuf {
    config_dir().join("tunnel.json")
}

#[cfg(unix)]
fn restrict_mode(file: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = fs::set_permissions(file, fs::Permissions::from_mode(0o600)) {
        crate::log_debug!("[tunnel] WARN chmod 0600 {}: {e}", file.display());
    }
}

#[cfg(not(unix))]
fn restrict_mode(_file: &Path) {}

/// Load the tunnel config from disk. A missing file yields
/// [`TunnelConfig::default`]; a malformed file is an error (fail loud —
/// we never silently fall back over a corrupt secret store).
pub fn load() -> Result<TunnelConfig, String> {
    let file = config_path();
    if !file.exists() {
        return Ok(TunnelConfig::default());
    }
    let raw = fs::read_to_string(&file)
        .map_err(|e| format!("read {}: {e}", file.display()))?;
    if raw.trim().is_empty() {
        return Ok(TunnelConfig::default());
    }
    serde_json::from_str(&raw)
        .map_err(|e| format!("parse {}: {e}", file.display()))
}

/// Persist the tunnel config via tmp+rename, then chmod 0600 so the
/// token is owner-only. Creates `~/.k2/` if absent.
pub fn save(cfg: &TunnelConfig) -> Result<(), String> {
    let dir = config_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let file = config_path();
    let tmp = dir.join(format!("tunnel.json.tmp.{}", std::process::id()));
    let body = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("serialize tunnel config: {e}"))?;
    fs::write(&tmp, body.as_bytes()).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    restrict_mode(&tmp);
    fs::rename(&tmp, &file).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename into place {}: {e}", file.display())
    })?;
    restrict_mode(&file);
    Ok(())
}

/// Read-modify-write helper: load, apply `f`, persist. The whole cycle
/// runs under the process lock so concurrent updates don't clobber.
pub fn update(f: impl FnOnce(&mut TunnelConfig)) -> Result<TunnelConfig, String> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _g = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let mut cfg = load()?;
    f(&mut cfg);
    save(&cfg)?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::test_support::with_temp_home;

    #[test]
    fn defaults_point_at_deployed_box() {
        let cfg = TunnelConfig::default();
        assert_eq!(cfg.server_addr, "178.156.232.105");
        assert_eq!(cfg.server_port, 7000);
        assert!(cfg.token.is_empty());
        assert!(cfg.local_port.is_none());
        assert!(!cfg.auto_start, "auto_start must default off");
    }

    #[test]
    fn auto_start_defaults_false_when_absent_from_json() {
        // A pre-auto_start tunnel.json (no field) must deserialize with
        // auto_start = false — never silently auto-dial after an upgrade.
        let cfg: TunnelConfig =
            serde_json::from_str(r#"{"token":"tok","subdomain":"rosson"}"#)
                .expect("parse legacy config");
        assert!(!cfg.auto_start);
        assert_eq!(cfg.token, "tok");
    }

    #[test]
    fn is_connectable_requires_token() {
        let mut cfg = TunnelConfig::default();
        assert!(!cfg.is_connectable(), "blank token must not be connectable");
        cfg.token = "tok_x".to_string();
        assert!(cfg.is_connectable());
    }

    #[test]
    fn public_url_uses_k2_dev_zone() {
        let mut cfg = TunnelConfig::default();
        assert_eq!(cfg.public_url(), None, "no subdomain -> no predicted URL");
        cfg.subdomain = "rosson".to_string();
        assert_eq!(cfg.public_url().unwrap(), "https://rosson.k2.dev");
    }

    #[test]
    fn load_returns_default_when_file_missing() {
        with_temp_home(|| {
            let cfg = load().expect("missing file must yield default, not error");
            assert_eq!(cfg, TunnelConfig::default());
        });
    }

    #[test]
    fn save_then_load_round_trips_including_token() {
        with_temp_home(|| {
            let cfg = TunnelConfig {
                server_addr: "1.2.3.4".to_string(),
                server_port: 7001,
                token: "tok_secret".to_string(),
                subdomain: "rosson".to_string(),
                local_port: Some(57839),
                auto_start: true,
                device_id: Some("dev-abc".to_string()),
                device_label: Some("MacIntel".to_string()),
                e2e: true,
            };
            save(&cfg).expect("save");
            let back = load().expect("load");
            assert_eq!(back, cfg, "config must round-trip byte-for-byte");
            assert_eq!(back.token, "tok_secret", "token must persist");
        });
    }

    #[cfg(unix)]
    #[test]
    fn saved_file_is_owner_only_0600() {
        use std::os::unix::fs::PermissionsExt;
        with_temp_home(|| {
            save(&TunnelConfig {
                token: "tok".to_string(),
                ..Default::default()
            })
            .expect("save");
            let mode = std::fs::metadata(config_path())
                .expect("stat tunnel.json")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "secret store must be chmod 0600, got {mode:o}");
        });
    }

    #[test]
    fn e2e_defaults_false_and_legacy_json_omitting_field_is_off() {
        // Default-constructed config must be OFF.
        assert!(!TunnelConfig::default().e2e, "e2e must default off");
        // A pre-e2e tunnel.json (no field) must deserialize with e2e=false —
        // never silently terminate TLS on the daemon after an upgrade.
        let cfg: TunnelConfig =
            serde_json::from_str(r#"{"token":"tok","subdomain":"rosson"}"#)
                .expect("parse legacy config");
        assert!(!cfg.e2e, "absent e2e field must deserialize false");
    }

    #[test]
    fn e2e_enabled_honors_config_field_and_env_var() {
        // Serialize with the HOME lock so the K2_E2E env mutation here can't
        // race the other env/HOME-touching suites.
        let _g = crate::themes::HOME_LOCK.lock();
        let prev = std::env::var_os("K2_E2E");
        std::env::remove_var("K2_E2E");

        let mut cfg = TunnelConfig::default();
        // Off by default: no env, e2e:false.
        assert!(!e2e_enabled(&cfg), "default config + no env must be OFF");

        // Config field flips it on.
        cfg.e2e = true;
        assert!(e2e_enabled(&cfg), "e2e:true config must enable");

        // Env var forces it on even when the config field is false.
        cfg.e2e = false;
        for truthy in ["1", "true", "TRUE", "Yes", "on"] {
            std::env::set_var("K2_E2E", truthy);
            assert!(
                e2e_enabled(&cfg),
                "K2_E2E={truthy} must enable regardless of config"
            );
        }
        // A falsey env value does not enable when the config is also false.
        for falsey in ["0", "false", "no", "off", ""] {
            std::env::set_var("K2_E2E", falsey);
            assert!(
                !e2e_enabled(&cfg),
                "K2_E2E={falsey:?} must NOT enable a non-e2e config"
            );
        }

        match prev {
            Some(p) => std::env::set_var("K2_E2E", p),
            None => std::env::remove_var("K2_E2E"),
        }
    }

    #[test]
    fn update_applies_and_persists() {
        with_temp_home(|| {
            let cfg = update(|c| {
                c.token = "tok_a".to_string();
                c.subdomain = "rosson".to_string();
            })
            .expect("update");
            assert_eq!(cfg.token, "tok_a");
            let reloaded = load().expect("load");
            assert_eq!(reloaded.subdomain, "rosson");
        });
    }
}
