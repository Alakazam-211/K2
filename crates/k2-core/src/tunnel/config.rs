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
//!   "server_addr": "relay.k2.dev",
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

/// Default frps relay (0.46.6): a DNS name K2 Web keeps pointed at the
/// current primary relay, flipped together with `*.k2.dev`. Host only —
/// the port is carried separately so the renderer can emit the two fields
/// frp's TOML schema expects. Before 0.46.6 this was relay k2e-01's IP
/// ([`LEGACY_DEFAULT_SERVER_HOST`]), which every saved `tunnel.json` still
/// carries; [`super::relay_list::effective`] replaces such a list in memory.
pub const DEFAULT_SERVER_HOST: &str = "relay.k2.dev";

/// The built-in default before 0.46.6 (relay k2e-01). Dialed only when the
/// `relay_default` switch says `legacy` ([`relay_default_mode`]). Remove
/// with the switch, in the first release after k2e-01 is retired.
pub const LEGACY_DEFAULT_SERVER_HOST: &str = "178.156.232.105";

/// Relay addresses older K2 builds baked in (the built-in default and the
/// provisioner's default list). A stored list made only of these hosts, on
/// port 7000, is "the old default", not a person's choice: 0.46.6 dials
/// [`DEFAULT_SERVER_HOST`] instead while that name resolves. Stays as a
/// migration table until no supported version can have written it.
pub const LEGACY_RELAY_HOSTS: &[&str] = &["178.156.232.105", "40.160.53.25"];

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

    /// 0.45.7 relay-list refresh: unix seconds when the daemon last wrote
    /// `relays` from the k2.dev relay list (`GET /api/tunnel/relays`, see
    /// [`super::relay_list`]). `None` = the list on disk was never fetched
    /// (provisioned by hand / the provisioner, or the built-in default).
    /// Written only together with a CHANGED list; an explicit `relays`
    /// write through `/cli/tunnel/config` clears it. Skipped when absent so
    /// older files keep their shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relays_fetched_at: Option<u64>,

    /// 0.46.3: which relay failover rules run (see
    /// [`super::failover::FailoverMode`] and [`failover_mode`]). Absent =
    /// the default, DNS-aware rules (move only when the `*.k2.dev` name
    /// can follow). `"legacy"` restores the 0.46.2 rules exactly. Kept a
    /// loose string so an unknown value can never make `tunnel.json`
    /// unreadable (it falls back to the default). Read at tunnel start;
    /// the env var `K2_TUNNEL_FAILOVER` overrides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failover: Option<String>,

    /// 0.46.6: what the built-in relay default is (see [`RelayDefault`] and
    /// [`relay_default_mode`]). Absent = `"name"`: the default relay is
    /// [`DEFAULT_SERVER_HOST`] and a stored list of only the old baked IPs
    /// is dialed as that name. `"legacy"` restores 0.46.5 exactly (the old
    /// IP, no replacement). A loose string like `failover`: an unknown
    /// value never makes `tunnel.json` unreadable. The env var
    /// `K2_RELAY_DEFAULT` overrides it. Removed with the legacy value in
    /// the first release after relay k2e-01 is retired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_default: Option<String>,

    /// 0.45.7: set by [`load`] when the token lives in the K2 vault and
    /// the vault could NOT be read (locked, unavailable, a transient
    /// keychain error). `token` is then empty, but that empty means
    /// "unknown", not "no token": [`save`] never clears the vault, and
    /// callers that need the token (start, relay-list fetch, release)
    /// treat it as a soft failure and retry later. In memory only — never
    /// serialized, never read from `tunnel.json`.
    /// prd-box-move-carry-v1 BM-A12: `"vault"` once the token lives in the
    /// K2 vault (written whenever the vault holder saves the config). While
    /// the vault is closed, [`load`] then says the token is locked in it
    /// (`token_read_error`) instead of looking unpaired.
    #[serde(default, rename = "tokenHome", skip_serializing_if = "Option::is_none")]
    pub token_home: Option<String>,

    #[serde(skip)]
    pub token_read_error: Option<String>,
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

/// The built-in relay host for a config that names none: the default name,
/// or the old IP when `K2_RELAY_DEFAULT=legacy` (the serde default can't
/// see the file's own `relay_default`; [`super::relay_list::effective`]
/// maps the name back to the old IP for that case).
pub fn default_server_host() -> String {
    match env_relay_default() {
        Some(RelayDefault::Legacy) => LEGACY_DEFAULT_SERVER_HOST.to_string(),
        _ => DEFAULT_SERVER_HOST.to_string(),
    }
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
            relays_fetched_at: None,
            failover: None,
            relay_default: None,
            token_home: None,
            token_read_error: None,
        }
    }
}

/// Env var overriding `tunnel.json` `relay_default` (`name` | `legacy`).
pub const RELAY_DEFAULT_ENV: &str = "K2_RELAY_DEFAULT";

/// 0.46.6: which built-in relay default this daemon uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RelayDefault {
    /// [`DEFAULT_SERVER_HOST`]; a stored list of only the old baked IPs is
    /// dialed as that name while it resolves (the default).
    #[default]
    Name,
    /// 0.46.5 exactly: the old IP is the default and nothing is replaced.
    Legacy,
}

impl RelayDefault {
    /// Lenient parse of the `tunnel.json` value / the env var. `None` =
    /// not recognised.
    pub fn parse(raw: &str) -> Option<RelayDefault> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "name" | "default" | "new" => Some(RelayDefault::Name),
            "legacy" | "old" | "0.46.5" => Some(RelayDefault::Legacy),
            _ => None,
        }
    }

    /// Wire / status name.
    pub fn as_str(self) -> &'static str {
        match self {
            RelayDefault::Name => "name",
            RelayDefault::Legacy => "legacy",
        }
    }
}

fn env_relay_default() -> Option<RelayDefault> {
    std::env::var(RELAY_DEFAULT_ENV)
        .ok()
        .and_then(|raw| RelayDefault::parse(&raw))
}

/// 0.46.6: what set the relay default in force: `"env"` (a recognised
/// [`RELAY_DEFAULT_ENV`]), `"tunnel.json"` (a recognised `relay_default`),
/// or `None` (neither: the built-in default).
pub fn relay_default_set_by(cfg: &TunnelConfig) -> Option<&'static str> {
    if env_relay_default().is_some() {
        Some("env")
    } else if cfg.relay_default.as_deref().and_then(RelayDefault::parse).is_some() {
        Some("tunnel.json")
    } else {
        None
    }
}

/// 0.46.6: the relay default in force. Precedence like [`failover_mode`]:
/// a recognised [`RELAY_DEFAULT_ENV`] wins, then a recognised
/// `tunnel.json` `relay_default`, else [`RelayDefault::Name`]. Unknown
/// values are ignored, never fatal.
pub fn relay_default_mode(cfg: &TunnelConfig) -> RelayDefault {
    if let Some(m) = env_relay_default() {
        return m;
    }
    cfg.relay_default
        .as_deref()
        .and_then(RelayDefault::parse)
        .unwrap_or_default()
}

/// Env var overriding `tunnel.json` `failover` (`legacy` | `dns`).
pub const FAILOVER_ENV: &str = "K2_TUNNEL_FAILOVER";

/// 0.46.3: the failover rules this tunnel runs. Precedence like
/// [`e2e_enabled`]: a recognised [`FAILOVER_ENV`] wins, then a recognised
/// `tunnel.json` `failover`, else the default
/// ([`super::failover::FailoverMode::Dns`]). Unrecognised values are
/// ignored (logged by the connector), never fatal.
pub fn failover_mode(cfg: &TunnelConfig) -> super::failover::FailoverMode {
    use super::failover::FailoverMode;
    if let Ok(raw) = std::env::var(FAILOVER_ENV) {
        if let Some(m) = FailoverMode::parse(&raw) {
            return m;
        }
    }
    cfg.failover
        .as_deref()
        .and_then(FailoverMode::parse)
        .unwrap_or_default()
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

    /// True when [`load`] could not read the vault-held token (see
    /// [`token_read_error`](TunnelConfig::token_read_error)): the token is
    /// unknown right now, which is NOT the same as unpaired.
    pub fn token_unreadable(&self) -> bool {
        self.token_read_error.is_some()
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
    crate::paths::home_dir()
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

/// Where the bearer token lives when the K2 vault is open (KV24): the
/// daemon installs a vault-backed holder at boot, after which
/// `tunnel.json` keeps every field except `token`. Every caller of
/// [`load`] still sees `cfg.token` (resolved in memory).
pub trait TokenVault: Send + Sync {
    /// `Ok(None)` = the vault holds no token; `Err` = the vault could not
    /// be read (locked, unavailable, a transient error). The two must stay
    /// apart: an `Err` folded into `None` reads as "unpaired" and a later
    /// save would wipe the real token (0.45.7).
    fn get(&self) -> Result<Option<String>, String>;
    fn set(&self, token: &str) -> Result<(), String>;
    fn clear(&self) -> Result<(), String>;
}

static TOKEN_VAULT: std::sync::OnceLock<Box<dyn TokenVault>> = std::sync::OnceLock::new();

/// [`TunnelConfig::token_home`] when the token is a vault secret.
pub const TOKEN_HOME_VAULT: &str = "vault";

/// Why the vault didn't open (its error code), set by the daemon at boot
/// and cleared when a vault opens (BM-A12).
static VAULT_CLOSED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// The daemon's vault didn't open (`Some(code)`, e.g. `vault_key_missing`)
/// or did (`None`).
pub fn set_vault_closed(code: Option<&str>) {
    *VAULT_CLOSED.lock().unwrap_or_else(|p| p.into_inner()) = code.map(str::to_string);
}

/// "the tunnel token is in the vault, which isn't open (vault_key_missing)".
pub fn vault_closed_line() -> String {
    let code = VAULT_CLOSED.lock().unwrap_or_else(|p| p.into_inner()).clone();
    format!(
        "the tunnel token is in the vault, which isn't open ({}); this device is still paired. k2 vault status says why (after a box move: k2 migrate import-secrets)",
        code.as_deref().unwrap_or("vault_not_open")
    )
}

/// Install the vault-backed token holder (once, at daemon boot).
pub fn install_token_vault(v: Box<dyn TokenVault>) -> bool {
    TOKEN_VAULT.set(v).is_ok()
}

/// Test-only stand-in for the vault holder (the real one is a process
/// `OnceLock` and lives in k2-daemon over the keychain). Scoped by
/// [`TestTokenVault`]; never consulted outside `cfg(test)`.
#[cfg(test)]
static TEST_VAULT: std::sync::Mutex<Option<std::sync::Arc<dyn TokenVault>>> =
    std::sync::Mutex::new(None);

/// Installs an in-memory token vault for one test; removed on drop.
/// Hold it inside a `TempHome` (the env lock) so tests never overlap.
#[cfg(test)]
pub(crate) struct TestTokenVault;

#[cfg(test)]
impl TestTokenVault {
    pub(crate) fn install(v: std::sync::Arc<dyn TokenVault>) -> Self {
        *TEST_VAULT.lock().unwrap_or_else(|p| p.into_inner()) = Some(v);
        TestTokenVault
    }
}

#[cfg(test)]
impl Drop for TestTokenVault {
    fn drop(&mut self) {
        *TEST_VAULT.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
}

/// In-memory stand-in for the K2 vault holder (desktops since 0.45.4 keep
/// the tunnel token there and `tunnel.json`'s token empty). `fail_reads`
/// makes `get` error like a locked / unavailable vault; writes still work
/// so a test can see whether anything tried to replace or clear the token.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct MemTokenVault {
    pub(crate) token: std::sync::Mutex<Option<String>>,
    pub(crate) fail_reads: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
impl MemTokenVault {
    pub(crate) fn holding(token: Option<&str>) -> std::sync::Arc<Self> {
        let v = Self::default();
        *v.token.lock().expect("mem vault") = token.map(str::to_string);
        std::sync::Arc::new(v)
    }
    pub(crate) fn set_fail_reads(&self, fail: bool) {
        self.fail_reads.store(fail, std::sync::atomic::Ordering::SeqCst);
    }
    /// What the vault holds, read past the failure switch.
    pub(crate) fn stored(&self) -> Option<String> {
        self.token.lock().expect("mem vault").clone()
    }
}

#[cfg(test)]
impl TokenVault for MemTokenVault {
    fn get(&self) -> Result<Option<String>, String> {
        if self.fail_reads.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("test vault: read failed (locked)".to_string());
        }
        Ok(self.stored())
    }
    fn set(&self, token: &str) -> Result<(), String> {
        *self.token.lock().expect("mem vault") = Some(token.to_string());
        Ok(())
    }
    fn clear(&self) -> Result<(), String> {
        *self.token.lock().expect("mem vault") = None;
        Ok(())
    }
}

/// Run `f` with the installed token vault (if any).
fn with_vault<R>(f: impl FnOnce(Option<&dyn TokenVault>) -> R) -> R {
    #[cfg(test)]
    {
        let test = TEST_VAULT.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some(v) = test {
            return f(Some(&*v));
        }
    }
    f(TOKEN_VAULT.get().map(|b| b.as_ref()))
}

/// Forget the vault-held token (Release / unpair).
pub fn clear_vault_token() -> Result<(), String> {
    with_vault(|v| match v {
        Some(v) => v.clear(),
        None => Ok(()),
    })
}

/// What the vault holds right now, for a rollback snapshot: `Ok(None)`
/// when no vault holder is installed, `Ok(Some(None))` when it holds no
/// token. An unreadable vault is an `Err` (never "no token").
pub(crate) fn vault_token() -> Result<Option<Option<String>>, String> {
    with_vault(|v| v.map(|v| v.get()).transpose())
}

/// Put a [`vault_token`] snapshot back: the old token, or no token when
/// there was none. A no-op without a vault holder.
pub(crate) fn restore_vault_token(token: Option<&str>) -> Result<(), String> {
    with_vault(|v| match (v, token) {
        (None, _) => Ok(()),
        (Some(v), Some(t)) => v.set(t),
        (Some(v), None) => v.clear(),
    })
}

/// The token as it sits in `tunnel.json` itself (adoption reads it once).
pub fn file_token() -> Result<String, String> {
    Ok(load_file()?.token)
}

/// Load the tunnel config from disk. A missing file yields
/// [`TunnelConfig::default`]; a malformed file is an error (fail loud —
/// we never silently fall back over a corrupt secret store).
///
/// A vault that can't be read is NOT an error here (every status read
/// would fail with it) and NOT an empty token: the config comes back with
/// `token` empty and [`TunnelConfig::token_read_error`] set, logged once
/// per failing stretch.
pub fn load() -> Result<TunnelConfig, String> {
    let mut cfg = load_file()?;
    if cfg.token.trim().is_empty() {
        match with_vault(|v| v.map(|v| v.get())) {
            // BM-A12: no holder because the vault didn't open, and the
            // token lives there: locked, not "not paired".
            None if cfg.token_home.as_deref() == Some(TOKEN_HOME_VAULT) => {
                let e = vault_closed_line();
                note_vault_read(Some(&e));
                cfg.token_read_error = Some(e);
            }
            None | Some(Ok(None)) => note_vault_read(None),
            Some(Ok(Some(t))) => {
                note_vault_read(None);
                cfg.token = t;
            }
            Some(Err(e)) => {
                note_vault_read(Some(&e));
                cfg.token_read_error = Some(e);
            }
        }
    }
    Ok(cfg)
}

/// Log a failed vault read once per failing stretch (status polls load
/// the config every few seconds), and its recovery once.
fn note_vault_read(err: Option<&str>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static FAILING: AtomicBool = AtomicBool::new(false);
    match err {
        Some(e) => {
            if !FAILING.swap(true, Ordering::SeqCst) {
                crate::log_debug!(
                    "[tunnel] could not read the tunnel token from the K2 vault ({e}); \
                     the stored token is kept and the tunnel left as it is — retrying later"
                );
            }
        }
        None => {
            if FAILING.swap(false, Ordering::SeqCst) {
                crate::log_debug!("[tunnel] the tunnel token reads from the K2 vault again");
            }
        }
    }
}

/// 0.46.6: the config as it sits in `tunnel.json`, WITHOUT resolving the
/// token from the K2 vault (so it never touches the keychain). For readers
/// that only need hosts, ports and switches: the mail PTR guard and the
/// tunnel's live relay-list check. A missing file is the default config.
/// `token` is whatever the file holds (empty when the vault has it).
pub fn load_without_token() -> Result<TunnelConfig, String> {
    load_file()
}

fn load_file() -> Result<TunnelConfig, String> {
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
///
/// With the vault holder installed, a non-empty `token` is written to the
/// vault (when it differs) and an EMPTY token leaves the vault untouched
/// (0.45.7): an empty token in memory may be a vault read that failed
/// ([`TunnelConfig::token_read_error`]), and treating it as "clear" let one
/// transient keychain error unpair the device at the next unrelated
/// config write. Forgetting the token is explicit: [`clear_vault_token`]
/// (Release / unpair).
pub fn save(cfg: &TunnelConfig) -> Result<(), String> {
    // With the vault holder installed, the token goes to the vault and the
    // file keeps everything else.
    let stripped;
    let vaulted = with_vault(|v| -> Result<bool, String> {
        match v {
            Some(v) => {
                let t = cfg.token.trim();
                if !t.is_empty() {
                    // A read error here just means "write it": the caller
                    // holds a real token (an unreadable load leaves it empty).
                    match v.get() {
                        Ok(Some(cur)) if cur.trim() == t => {}
                        _ => v.set(t)?,
                    }
                }
                Ok(true)
            }
            None => Ok(false),
        }
    })?;
    let cfg = if vaulted {
        let mut c = cfg.clone();
        c.token = String::new();
        c.token_home = Some(TOKEN_HOME_VAULT.to_string());
        stripped = c;
        &stripped
    } else {
        cfg
    };
    let dir = config_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let file = config_path();
    let tmp = dir.join(format!("tunnel.json.tmp.{}", std::process::id()));
    let body = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("serialize tunnel config: {e}"))?;
    // Unchanged content → no write (0.45.7): re-applying the same config
    // (a tunnel start, a device-id push, a same-identity config POST) must
    // not touch tunnel.json, so its mtime only moves when something
    // actually changed. The mode is still enforced.
    if fs::read(&file).is_ok_and(|on_disk| on_disk == body.as_bytes()) {
        restrict_mode(&file);
        return Ok(());
    }
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
    update_if(|c| {
        f(c);
        true
    })
}

/// [`update`] that persists ONLY when `f` returns true (it changed
/// something). Same process lock, so a conditional write can't interleave
/// with another update. Returns the (possibly unchanged) config.
pub fn update_if(f: impl FnOnce(&mut TunnelConfig) -> bool) -> Result<TunnelConfig, String> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _g = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let mut cfg = load()?;
    if f(&mut cfg) {
        save(&cfg)?;
    }
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::test_support::with_temp_home;

    #[test]
    fn defaults_point_at_the_relay_name() {
        let _g = crate::test_env::lock();
        let _e = crate::test_env::EnvVar::remove(RELAY_DEFAULT_ENV);
        let cfg = TunnelConfig::default();
        assert_eq!(cfg.server_addr, "relay.k2.dev");
        assert!(
            super::super::relay_list::valid_host(&cfg.server_addr),
            "the default must pass the relay-list host check"
        );
        assert_eq!(cfg.server_port, 7000);
        let parsed: TunnelConfig =
            serde_json::from_str(r#"{"token":"t"}"#).expect("a file without server_addr");
        assert_eq!(parsed.server_addr, "relay.k2.dev", "the serde default is the name too");
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
                relays_fetched_at: Some(1_792_060_000),
                failover: Some("legacy".to_string()),
                relay_default: Some("legacy".to_string()),
                token_home: None,
                token_read_error: None,
            };
            save(&cfg).expect("save");
            let back = load().expect("load");
            assert_eq!(back, cfg, "config must round-trip byte-for-byte");
            assert_eq!(back.token, "tok_secret", "token must persist");
        });
    }

    #[test]
    fn save_with_unchanged_content_does_not_rewrite_the_file() {
        with_temp_home(|| {
            let cfg = TunnelConfig {
                subdomain: "afs".to_string(),
                token: "tok".to_string(),
                ..Default::default()
            };
            save(&cfg).expect("first save");
            let mtime = || {
                fs::metadata(config_path()).expect("tunnel.json").modified().expect("mtime")
            };
            let first = mtime();
            std::thread::sleep(std::time::Duration::from_millis(50));
            save(&cfg).expect("same save");
            update(|_| {}).expect("no-op update");
            assert_eq!(mtime(), first, "unchanged content must not touch tunnel.json");

            save(&TunnelConfig { auto_start: true, ..cfg }).expect("changed save");
            assert_ne!(mtime(), first, "a real change is written");
            assert!(load().expect("load").auto_start);
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
    fn failover_mode_default_config_field_and_env_precedence() {
        use super::super::failover::FailoverMode;
        let _unset = crate::test_env::EnvVar::remove(FAILOVER_ENV);
        let mut cfg = TunnelConfig::default();
        assert_eq!(failover_mode(&cfg), FailoverMode::Dns, "DNS-aware is the default");
        cfg.failover = Some("legacy".to_string());
        assert_eq!(failover_mode(&cfg), FailoverMode::Legacy, "tunnel.json switch");
        cfg.failover = Some("nonsense".to_string());
        assert_eq!(failover_mode(&cfg), FailoverMode::Dns, "unknown value → default");
        {
            let _v = crate::test_env::EnvVar::set(FAILOVER_ENV, "legacy");
            assert_eq!(failover_mode(&cfg), FailoverMode::Legacy, "env wins");
        }
        cfg.failover = Some("legacy".to_string());
        {
            let _v = crate::test_env::EnvVar::set(FAILOVER_ENV, "dns");
            assert_eq!(failover_mode(&cfg), FailoverMode::Dns, "env wins both ways");
        }
        {
            let _v = crate::test_env::EnvVar::set(FAILOVER_ENV, "maybe");
            assert_eq!(failover_mode(&cfg), FailoverMode::Legacy, "bad env → config");
        }
        // Round trip: absent stays absent on disk; a legacy switch persists
        // and an unknown value never breaks parsing.
        let json = serde_json::to_string(&TunnelConfig::default()).expect("ser");
        assert!(!json.contains("failover"), "{json}");
        let back: TunnelConfig =
            serde_json::from_str(r#"{"token":"t","failover":"legacy"}"#).expect("de");
        assert_eq!(back.failover.as_deref(), Some("legacy"));
        let odd: TunnelConfig =
            serde_json::from_str(r#"{"token":"t","failover":"???"}"#).expect("de odd");
        assert_eq!(failover_mode(&odd), FailoverMode::Dns);
    }

    #[test]
    fn e2e_enabled_default_on_config_field_and_env_precedence() {
        // Serialize with the HOME lock so the K2_E2E env mutation here can't
        // race the other env/HOME-touching suites.
        let _g = crate::themes::HOME_LOCK.lock();
        // Guards restore K2_E2E on drop (LIFO), also on panic.
        let _unset = crate::test_env::EnvVar::remove("K2_E2E");

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
            let _v = crate::test_env::EnvVar::set("K2_E2E", truthy);
            assert!(
                e2e_enabled(&cfg),
                "K2_E2E={truthy} must force ON regardless of config"
            );
        }
        // A falsey env var forces OFF even when the config field is true
        // (env-level opt-out wins over a default-on / explicit-on config).
        cfg.e2e = true;
        for falsey in ["0", "false", "no", "off", ""] {
            let _v = crate::test_env::EnvVar::set("K2_E2E", falsey);
            assert!(
                !e2e_enabled(&cfg),
                "K2_E2E={falsey:?} must force OFF even over an e2e:true config"
            );
        }
        // An unrecognised env value is no override → follows the config.
        let _maybe = crate::test_env::EnvVar::set("K2_E2E", "maybe");
        assert!(
            e2e_enabled(&cfg),
            "unrecognised K2_E2E must defer to the (true) config"
        );
        cfg.e2e = false;
        assert!(
            !e2e_enabled(&cfg),
            "unrecognised K2_E2E must defer to the (false) config"
        );
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
    fn default_config_relay_list_is_the_relay_name() {
        let _g = crate::test_env::lock();
        let _e = crate::test_env::EnvVar::remove(RELAY_DEFAULT_ENV);
        assert_eq!(DEFAULT_SERVER_HOST, "relay.k2.dev");
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

    fn on_disk() -> TunnelConfig {
        serde_json::from_str(&std::fs::read_to_string(config_path()).expect("read tunnel.json"))
            .expect("parse tunnel.json")
    }

    fn seed_vaulted(vault: &MemTokenVault, token: &str) {
        save(&TunnelConfig {
            token: token.to_string(),
            subdomain: "rosson".to_string(),
            ..Default::default()
        })
        .expect("seed");
        assert_eq!(vault.stored().as_deref(), Some(token), "setup: token in the vault");
        assert_eq!(on_disk().token, "", "setup: tunnel.json keeps no token");
    }

    /// prd-box-move-carry-v1 BM-A12: a box whose vault didn't open (a /home
    /// copy without its key) says the token is locked in the vault, not
    /// "not paired"; a box whose token was never vaulted stays as before.
    #[test]
    fn a_closed_vault_holding_the_token_reads_as_locked_not_unpaired() {
        with_temp_home(|| {
            {
                let vault = MemTokenVault::holding(None);
                let _v = TestTokenVault::install(vault.clone());
                seed_vaulted(&vault, "tok_in_the_vault");
                assert_eq!(on_disk().token_home.as_deref(), Some(TOKEN_HOME_VAULT), "the marker is written with the vaulted save");
            }
            // The daemon booted with its vault closed: no holder installed.
            set_vault_closed(Some("vault_key_missing"));
            let cfg = load().expect("load");
            // The stopped status says so too.
            let st = super::super::connector::status();
            set_vault_closed(None);
            assert_eq!(cfg.token, "");
            let e = cfg.token_read_error.as_deref().expect("locked, not empty");
            assert!(e.contains("in the vault, which isn't open (vault_key_missing)") && e.contains("still paired"), "{e}");
            assert!(!cfg.is_connectable());
            assert!(!st.running);
            assert_eq!(st.token_locked.as_deref(), Some(e));
        });
        with_temp_home(|| {
            // A plaintext-token box (never vaulted) never says "locked".
            save(&TunnelConfig { token: String::new(), subdomain: "rosson".to_string(), ..Default::default() }).expect("save");
            let cfg = load().expect("load");
            assert!(cfg.token_read_error.is_none());
            assert!(on_disk().token_home.is_none());
        });
    }

    #[test]
    fn an_unreadable_vault_loads_as_unknown_not_as_no_token() {
        with_temp_home(|| {
            let vault = MemTokenVault::holding(None);
            let _v = TestTokenVault::install(vault.clone());
            seed_vaulted(&vault, "tok_kept");
            vault.set_fail_reads(true);
            let cfg = load().expect("an unreadable vault is not a load error");
            assert_eq!(cfg.token, "");
            assert!(cfg.token_unreadable(), "the read failure must be carried");
            assert!(
                cfg.token_read_error.as_deref().is_some_and(|e| e.contains("read failed")),
                "{:?}",
                cfg.token_read_error
            );
            assert!(!cfg.is_connectable());
            // Never serialized.
            let json = serde_json::to_string(&cfg).expect("serialize");
            assert!(!json.contains("token_read_error") && !json.contains("read failed"), "{json}");

            vault.set_fail_reads(false);
            let back = load().expect("load");
            assert_eq!(back.token, "tok_kept");
            assert!(!back.token_unreadable());

            // An empty vault is "no token", not a read failure.
            vault.clear().expect("clear");
            assert!(!load().expect("load").token_unreadable());
        });
    }

    #[test]
    fn an_update_during_a_failed_vault_read_keeps_the_stored_token() {
        with_temp_home(|| {
            let vault = MemTokenVault::holding(None);
            let _v = TestTokenVault::install(vault.clone());
            seed_vaulted(&vault, "tok_kept");
            vault.set_fail_reads(true);

            let relays = vec![
                RelayEndpoint { host: "10.0.0.1".to_string(), port: 7000 },
                RelayEndpoint { host: "10.0.0.2".to_string(), port: 7000 },
            ];
            let cfg = update(|c| c.relays = relays.clone()).expect("unrelated update");
            assert!(cfg.token_unreadable());
            assert_eq!(on_disk().relays, relays, "the unrelated field is written");
            assert_eq!(
                vault.stored().as_deref(),
                Some("tok_kept"),
                "a failed vault read must never clear the stored token"
            );
            update(|c| c.enabled = false).expect("another update");
            update_if(|c| {
                c.auto_start = true;
                true
            })
            .expect("update_if");
            assert_eq!(vault.stored().as_deref(), Some("tok_kept"));
            assert_eq!(on_disk().token, "", "the token never lands in tunnel.json");

            vault.set_fail_reads(false);
            let back = load().expect("load");
            assert_eq!(back.token, "tok_kept", "the token survives the failing stretch");
            assert_eq!(back.relays, relays);
            assert!(!back.enabled && back.auto_start);
        });
    }

    #[test]
    fn saving_an_empty_token_never_clears_the_vault() {
        with_temp_home(|| {
            let vault = MemTokenVault::holding(None);
            let _v = TestTokenVault::install(vault.clone());
            seed_vaulted(&vault, "tok_kept");
            save(&TunnelConfig { subdomain: "rosson".to_string(), ..Default::default() })
                .expect("save without a token");
            assert_eq!(vault.stored().as_deref(), Some("tok_kept"));
            assert_eq!(load().expect("load").token, "tok_kept");
        });
    }

    #[test]
    fn pairing_a_new_token_replaces_the_vault_token_even_when_reads_fail() {
        with_temp_home(|| {
            let vault = MemTokenVault::holding(None);
            let _v = TestTokenVault::install(vault.clone());
            seed_vaulted(&vault, "tok_old");
            // Desktop pairing: POST /cli/tunnel/config with the new token.
            super::super::set_config(super::super::TunnelConfigUpdate {
                token: Some("tok_new".to_string()),
                ..Default::default()
            })
            .expect("pair");
            assert_eq!(vault.stored().as_deref(), Some("tok_new"));
            assert_eq!(load().expect("load").token, "tok_new");

            // An explicit new token is written even while reads fail.
            vault.set_fail_reads(true);
            update(|c| c.token = "tok_newer".to_string()).expect("pair again");
            assert_eq!(vault.stored().as_deref(), Some("tok_newer"));
            assert_eq!(on_disk().token, "", "the token never lands in tunnel.json");
        });
    }

    #[test]
    fn release_clears_the_vault_token_and_refuses_while_the_vault_is_unreadable() {
        use super::super::unpair::{perform_release_mode, ReleaseMode};
        with_temp_home(|| {
            let vault = MemTokenVault::holding(None);
            let _v = TestTokenVault::install(vault.clone());
            seed_vaulted(&vault, "tok_kept");

            vault.set_fail_reads(true);
            let err = perform_release_mode(ReleaseMode::ForgetLocal)
                .expect_err("a release without a readable token must refuse");
            assert!(err.contains("could not be read from the K2 vault"), "{err}");
            assert_eq!(vault.stored().as_deref(), Some("tok_kept"), "nothing released");
            assert!(config_path().exists(), "tunnel.json kept");

            vault.set_fail_reads(false);
            let report = perform_release_mode(ReleaseMode::ForgetLocal).expect("release");
            assert!(report.local_only);
            assert_eq!(vault.stored(), None, "release (unpair) clears the vault token");
            assert!(!config_path().exists(), "release deletes tunnel.json");
            assert_eq!(load().expect("load").token, "");
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
