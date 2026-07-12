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
//!
//! **Multi-relay failover:** an optional ordered `relays` array
//! (`[{"host": "...", "port": 7000}, ...]`) supersedes the single
//! `server_addr`/`server_port` pair when present — index 0 is the
//! preferred relay, the rest are fallbacks. Absent (every pre-failover
//! file) the legacy pair IS the list; see [`TunnelConfig::relay_list`].

use std::fmt;
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

/// One frps relay endpoint (host + frpc dial-in port) in the ordered
/// fallback list. The edge relays are PEERS — they share one frps auth
/// token and one wildcard cert — so **only the address differs** between
/// them; token/subdomain/proxy-name never change on a relay switch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelayEndpoint {
    /// frps host (IP or DNS name, no port).
    pub host: String,
    /// frps `bindPort`. Defaults to [`DEFAULT_SERVER_PORT`] so a relay
    /// entry can be written as `{"host": "..."}` alone.
    #[serde(default = "default_server_port")]
    pub port: u16,
}

impl fmt::Display for RelayEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

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

    /// Ordered relay fallback list (edge resilience). When non-empty this
    /// is AUTHORITATIVE: index 0 is the preferred/primary relay, later
    /// entries are fallbacks the connector rotates through on repeated
    /// dial failure (and fails back from when the primary recovers).
    ///
    /// **Backward compatible:** an existing single-endpoint `tunnel.json`
    /// simply omits this field — it deserializes to an empty list, and
    /// [`relay_list`](TunnelConfig::relay_list) folds the legacy
    /// `server_addr`/`server_port` pair into a one-element list. Empty is
    /// also skipped on serialize so a legacy config re-saved by the daemon
    /// keeps its old on-disk shape byte-for-byte.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relays: Vec<RelayEndpoint>,

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

    /// **K2 Connect E2E encryption (PRD `k2-connect-e2e-encryption.md` §4
    /// Option A) — ON BY DEFAULT, user-opt-out, effective 0.40.6+.** When
    /// true the daemon terminates TLS itself (a rustls listener presents a
    /// broker-issued cert for `<sub>.k2.dev`) and frpc forwards the
    /// *encrypted* stream (`type = "https"`) so the relay carries only
    /// ciphertext. When false — the user's explicit **opt-out** — behaviour
    /// is the legacy terminating path: frpc `type = "http"` and Caddy
    /// terminates TLS at the relay.
    ///
    /// **Default is `true`** (see [`default_e2e`]): a config that omits the
    /// field — including an existing 0.40.5 `tunnel.json` upgraded in place —
    /// deserializes with `e2e: true`, so E2E is on without the user ever
    /// turning it on. A user who *explicitly* writes `e2e: false` keeps the
    /// plaintext path; that opt-out is honoured and must keep working.
    ///
    /// The env var `K2_E2E` overrides the stored value at runtime (see
    /// [`e2e_enabled`]): a falsey value forces OFF (an env-level opt-out), a
    /// truthy value forces ON, and unset → follow this field.
    #[serde(default = "default_e2e")]
    pub e2e: bool,

    /// **Disable-vs-Release PRD (`prd-tunnel-disable-unpair-v1.md` §2A) —
    /// the persisted PAUSE flag.** `false` = the user disabled the tunnel
    /// on this device: frpc must never spawn, across daemon restarts,
    /// machine reboots, and orphaned daemons alike. The gate lives at the
    /// frpc-SPAWN site (`connector::spawn_gate`), which re-reads this
    /// field from DISK on every spawn attempt — no cached copy, so a
    /// second/orphaned daemon respects a disable it never saw happen.
    ///
    /// Defaults `true` (a config that omits the field — every pre-PRD
    /// `tunnel.json` — stays enabled), and the default is skipped on
    /// serialize so an enabled config keeps its old on-disk shape.
    /// Identity (token, subdomain, lease) is untouched by this flag —
    /// disable is the reversible pause; Release (`unpair`) is the
    /// destructive divorce.
    #[serde(default = "default_enabled", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

/// True when K2 Connect end-to-end encryption (Option A) is enabled for
/// this daemon. The single gate every E2E code path consults.
///
/// **Precedence (0.40.6+, default-ON + opt-out):**
///   1. An *explicit* env opt-out wins → OFF. `K2_E2E` in
///      `{0,false,no,off}` (case-insensitive) forces E2E off regardless of
///      the stored config — an operator-level kill switch / opt-out.
///   2. An *explicit* env opt-in → ON. `K2_E2E` in `{1,true,yes,on}` forces
///      E2E on regardless of the stored config.
///   3. Otherwise (env unset or unrecognised) → follow the config field,
///      which **defaults to `true`** ([`default_e2e`]). So a user who never
///      expressed a preference gets E2E ON; a user who explicitly persisted
///      `e2e: false` gets the plaintext opt-out path.
///
/// Net effect: E2E is the default for everyone; turning it OFF requires an
/// explicit `e2e: false` in `tunnel.json` OR a falsey `K2_E2E` — you never
/// have to turn it ON.
pub fn e2e_enabled(cfg: &TunnelConfig) -> bool {
    match env_e2e_override() {
        Some(forced) => forced, // explicit env opt-in/opt-out wins
        None => cfg.e2e,        // no env preference → config (defaults true)
    }
}

/// Default for the `e2e` config field when it's absent from `tunnel.json`:
/// **true** (E2E ON by default, 0.40.6+). A custom serde default fn (rather
/// than `#[serde(default)]`, which would give `false`) so an existing config
/// that predates this field upgrades to E2E-on automatically.
fn default_e2e() -> bool {
    true
}

/// Parse the `K2_E2E` env var as an explicit override, if present and
/// recognised:
///   * `Some(true)`  for `1`/`true`/`yes`/`on`  (force ON / opt-in)
///   * `Some(false)` for `0`/`false`/`no`/`off`/`` (force OFF / opt-out)
///   * `None` when the var is unset or holds an unrecognised value (defer to
///     the config). The empty string is treated as an explicit OFF so
///     `K2_E2E=` reads as "off" rather than silently deferring to a now
///     default-on config.
///
/// Isolated so the parsing rule lives in exactly one place and is unit-tested.
fn env_e2e_override() -> Option<bool> {
    let raw = std::env::var("K2_E2E").ok()?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" | "" => Some(false),
        _ => None, // unrecognised → no override, follow config
    }
}

/// Default for the `enabled` pause flag when absent from `tunnel.json`:
/// **true** — every pre-PRD config (no field) stays enabled. Only an
/// explicit `"enabled": false` (written by `k2 tunnel disable` / the
/// Settings toggle / `POST /cli/tunnel/disable`) pauses the tunnel.
fn default_enabled() -> bool {
    true
}

/// serde `skip_serializing_if` helper: skip the `enabled` field when it
/// holds the default (`true`) so an enabled config keeps the legacy
/// on-disk shape byte-for-byte.
fn is_true(b: &bool) -> bool {
    *b
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
            relays: Vec::new(),
            token: String::new(),
            subdomain: String::new(),
            local_port: None,
            auto_start: false,
            device_id: None,
            device_label: None,
            // E2E ON by default (0.40.6+); user opts out via `e2e: false`.
            e2e: default_e2e(),
            // Enabled by default — `false` is the persisted PAUSE
            // (PRD tunnel-disable-unpair §2A).
            enabled: default_enabled(),
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

    /// The effective ordered relay list — ALWAYS at least one entry.
    ///
    /// * `relays` non-empty → that list verbatim (index 0 = primary).
    /// * `relays` empty (every pre-failover `tunnel.json`) → the legacy
    ///   single `server_addr`/`server_port` pair as a one-element list,
    ///   so single-relay behaviour is byte-identical to before.
    pub fn relay_list(&self) -> Vec<RelayEndpoint> {
        if self.relays.is_empty() {
            vec![RelayEndpoint {
                host: self.server_addr.clone(),
                port: self.server_port,
            }]
        } else {
            self.relays.clone()
        }
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
                relays: vec![
                    RelayEndpoint { host: "1.2.3.4".to_string(), port: 7001 },
                    RelayEndpoint { host: "5.6.7.8".to_string(), port: 7000 },
                ],
                token: "tok_secret".to_string(),
                subdomain: "rosson".to_string(),
                local_port: Some(57839),
                auto_start: true,
                device_id: Some("dev-abc".to_string()),
                device_label: Some("MacIntel".to_string()),
                e2e: true,
                enabled: true,
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
    fn e2e_defaults_on_and_legacy_json_omitting_field_is_on() {
        // 0.40.6+: E2E is ON by default. Default-constructed config is ON.
        assert!(TunnelConfig::default().e2e, "e2e must default ON (0.40.6+)");
        // A pre-e2e / 0.40.5 tunnel.json (no field) must deserialize with
        // e2e=true — an existing user upgrades into E2E without touching
        // anything (default-on; they never have to turn it on).
        let cfg: TunnelConfig =
            serde_json::from_str(r#"{"token":"tok","subdomain":"rosson"}"#)
                .expect("parse legacy config");
        assert!(cfg.e2e, "absent e2e field must deserialize true (default-on)");
    }

    #[test]
    fn e2e_explicit_false_in_json_is_the_opt_out() {
        // The opt-out: a user who EXPLICITLY sets e2e:false keeps the
        // plaintext/terminating path. The explicit false must survive
        // deserialization (not be coerced back to the default).
        let cfg: TunnelConfig =
            serde_json::from_str(r#"{"token":"tok","subdomain":"rosson","e2e":false}"#)
                .expect("parse opt-out config");
        assert!(!cfg.e2e, "explicit e2e:false must deserialize false (opt-out)");
    }

    #[test]
    fn e2e_enabled_default_on_config_field_and_env_precedence() {
        // Serialize with the HOME lock so the K2_E2E env mutation here can't
        // race the other env/HOME-touching suites.
        let _g = crate::themes::HOME_LOCK.lock();
        let prev = std::env::var_os("K2_E2E");
        std::env::remove_var("K2_E2E");

        let mut cfg = TunnelConfig::default();
        // ON by default: no env, e2e defaults true.
        assert!(e2e_enabled(&cfg), "default config + no env must be ON (0.40.6+)");

        // The opt-out: explicit e2e:false with no env → OFF.
        cfg.e2e = false;
        assert!(
            !e2e_enabled(&cfg),
            "explicit e2e:false + no env must be OFF (the opt-out)"
        );

        // A truthy env var forces ON even over the opt-out config.
        for truthy in ["1", "true", "TRUE", "Yes", "on"] {
            std::env::set_var("K2_E2E", truthy);
            assert!(
                e2e_enabled(&cfg),
                "K2_E2E={truthy} must force ON regardless of config"
            );
        }
        // A falsey env var forces OFF even when the config field is true
        // (env-level opt-out wins over a default-on / explicit-on config).
        cfg.e2e = true;
        for falsey in ["0", "false", "no", "off", ""] {
            std::env::set_var("K2_E2E", falsey);
            assert!(
                !e2e_enabled(&cfg),
                "K2_E2E={falsey:?} must force OFF even over an e2e:true config"
            );
        }
        // An unrecognised env value is no override → follows the config.
        std::env::set_var("K2_E2E", "maybe");
        assert!(
            e2e_enabled(&cfg),
            "unrecognised K2_E2E must defer to the (true) config"
        );
        cfg.e2e = false;
        assert!(
            !e2e_enabled(&cfg),
            "unrecognised K2_E2E must defer to the (false) config"
        );

        match prev {
            Some(p) => std::env::set_var("K2_E2E", p),
            None => std::env::remove_var("K2_E2E"),
        }
    }

    #[test]
    fn legacy_single_endpoint_json_reads_as_one_element_relay_list() {
        // A pre-failover tunnel.json (no `relays` key) must deserialize
        // UNCHANGED — and the effective relay list must be exactly the
        // legacy server_addr/server_port pair.
        let cfg: TunnelConfig = serde_json::from_str(
            r#"{"server_addr":"1.2.3.4","server_port":7001,"token":"tok","subdomain":"rosson"}"#,
        )
        .expect("parse legacy single-endpoint config");
        assert!(cfg.relays.is_empty(), "absent relays field must stay empty");
        assert_eq!(
            cfg.relay_list(),
            vec![RelayEndpoint { host: "1.2.3.4".to_string(), port: 7001 }],
            "legacy config must fold into a one-element relay list"
        );
    }

    #[test]
    fn relays_array_json_parses_ordered_and_port_defaults() {
        // The new shape: an ordered `relays` array. Order is load-bearing
        // (index 0 = primary) and a host-only entry gets the default port.
        let cfg: TunnelConfig = serde_json::from_str(
            r#"{
                "token": "tok",
                "subdomain": "rosson",
                "relays": [
                    {"host": "10.0.0.1", "port": 7001},
                    {"host": "10.0.0.2"}
                ]
            }"#,
        )
        .expect("parse multi-relay config");
        assert_eq!(
            cfg.relay_list(),
            vec![
                RelayEndpoint { host: "10.0.0.1".to_string(), port: 7001 },
                RelayEndpoint { host: "10.0.0.2".to_string(), port: DEFAULT_SERVER_PORT },
            ],
            "relays must parse in order with the port defaulting"
        );
    }

    #[test]
    fn default_config_relay_list_is_the_deployed_box() {
        assert_eq!(
            TunnelConfig::default().relay_list(),
            vec![RelayEndpoint {
                host: DEFAULT_SERVER_HOST.to_string(),
                port: DEFAULT_SERVER_PORT,
            }]
        );
    }

    #[test]
    fn legacy_config_resave_does_not_grow_a_relays_key() {
        // A legacy (relays-empty) config re-saved by the daemon must keep
        // its old on-disk shape — no `"relays": []` noise appearing in a
        // file the user/provisioner may diff or hand-edit.
        with_temp_home(|| {
            save(&TunnelConfig {
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                ..Default::default()
            })
            .expect("save legacy-shaped config");
            let raw = std::fs::read_to_string(config_path()).expect("read tunnel.json");
            assert!(
                !raw.contains("relays"),
                "empty relay list must be skipped on serialize\n{raw}"
            );
        });
    }

    #[test]
    fn multi_relay_config_round_trips() {
        with_temp_home(|| {
            let cfg = TunnelConfig {
                relays: vec![
                    RelayEndpoint { host: "10.0.0.1".to_string(), port: 7000 },
                    RelayEndpoint { host: "10.0.0.2".to_string(), port: 7000 },
                ],
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                ..Default::default()
            };
            save(&cfg).expect("save");
            let back = load().expect("load");
            assert_eq!(back, cfg, "multi-relay config must round-trip");
            assert_eq!(back.relay_list().len(), 2);
        });
    }

    #[test]
    fn enabled_defaults_true_and_legacy_json_is_enabled() {
        // PRD tunnel-disable-unpair §2A: the pause flag defaults ON so
        // every pre-PRD tunnel.json (no field) keeps working untouched.
        assert!(TunnelConfig::default().enabled, "enabled must default true");
        let cfg: TunnelConfig =
            serde_json::from_str(r#"{"token":"tok","subdomain":"rosson"}"#)
                .expect("parse legacy config");
        assert!(cfg.enabled, "absent enabled field must deserialize true");
    }

    #[test]
    fn disable_persists_across_simulated_restart() {
        // The incident's exact failure, as a state-file round-trip: a
        // persisted disable must survive a fresh process reading the file
        // cold (daemon restart / reboot / orphaned second daemon).
        with_temp_home(|| {
            save(&TunnelConfig {
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                enabled: false,
                ..Default::default()
            })
            .expect("save disabled config");
            // Simulated restart = a cold load with no in-memory state.
            let back = load().expect("reload after 'restart'");
            assert!(!back.enabled, "disable must survive the round-trip");
            // The flag is IN the file, not implied — a raw read shows it.
            let raw = std::fs::read_to_string(config_path()).expect("read tunnel.json");
            assert!(
                raw.contains("\"enabled\": false"),
                "disable must be persisted on disk, not in-memory\n{raw}"
            );
            // Re-enable is symmetric.
            update(|c| c.enabled = true).expect("re-enable");
            assert!(load().expect("reload").enabled, "re-enable must persist");
        });
    }

    #[test]
    fn enabled_config_resave_keeps_legacy_shape() {
        // An enabled (default) config re-saved by the daemon must not grow
        // an `"enabled": true` key — same contract as the empty relays list.
        with_temp_home(|| {
            save(&TunnelConfig {
                token: "tok".to_string(),
                subdomain: "rosson".to_string(),
                ..Default::default()
            })
            .expect("save enabled config");
            let raw = std::fs::read_to_string(config_path()).expect("read tunnel.json");
            assert!(
                !raw.contains("enabled"),
                "default (enabled) must be skipped on serialize\n{raw}"
            );
        });
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
