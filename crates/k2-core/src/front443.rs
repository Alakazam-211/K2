//! front443's owner choice (`~/.k2/front443.json`), shared by the daemon's
//! SNI router and the k2-core pieces that move behind it (Skin Direct's
//! Caddy, `skin_door`). The router itself lives in `k2-daemon::front443`.
//!
//! - `enabled`: the box's :443 belongs to the daemon's front (the root
//!   helper's `k2-front-443.socket`). Skin Direct's Caddy then serves its
//!   Host sites on loopback [`CADDY_FRONT_PORT`] instead of claiming :443.
//! - `mail`: opt-in — Stalwart's HTTPS listener moves to 127.0.0.1:8443
//!   behind the front (pass-through, PROXY v2), keeping ACME TLS-ALPN-01.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Skin Direct's Caddy HTTPS port behind the front (loopback only).
pub const CADDY_FRONT_PORT: u16 = 38473;

/// The daemon's App TLS door behind the front (0.46.6,
/// prd-app-custom-domains-v1 D1, vs-live CD-A7): loopback only, a FIXED
/// port so a hand-off successor's table (fixed backends only) keeps the
/// custom App names routed from its first connection.
pub const APP_DOOR_PORT: u16 = 38474;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FrontConfig {
    pub enabled: bool,
    /// Stalwart behind the front (only meaningful with `enabled`).
    pub mail: bool,
    pub changed_at: Option<i64>,
}

impl FrontConfig {
    pub fn mail_behind_front(&self) -> bool {
        self.enabled && self.mail
    }
}

pub fn path() -> PathBuf {
    crate::paths::k2_home().join("front443.json")
}

/// The saved choice; a missing or unreadable file is "off".
pub fn load() -> FrontConfig {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save(c: &FrontConfig) -> Result<(), String> {
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("mkdir {}: {e}", d.display()))?;
    }
    let text = serde_json::to_string_pretty(c).map_err(|e| e.to_string())?;
    crate::fs_atomic::atomic_write_private(&p, text.as_bytes()).map_err(|e| format!("write {}: {e}", p.display()))
}

/// Skin Direct's Caddy sits behind the front.
pub fn caddy_behind_front() -> bool {
    load().enabled
}

/// Stalwart sits behind the front.
pub fn mail_behind_front() -> bool {
    load().mail_behind_front()
}

// ── owner passthrough names (0.46.6) ─────────────────────────────────

/// A name the front doesn't own, passed through by SNI to a web server the
/// owner already runs on this box (`k2 front passthrough add`): e.g. a
/// system Caddy or nginx moved from :443 to `127.0.0.1:9443`, so its sites
/// keep working untouched while the front holds :443. `host` `*` is the
/// default route (every name K2 doesn't route itself). TLS is never
/// terminated: the upstream keeps its own certificates (its TLS-ALPN-01
/// hello passes through too).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Passthrough {
    pub host: String,
    pub to: std::net::SocketAddr,
    /// Write a PROXY v2 header first (only if the upstream reads one).
    #[serde(default)]
    pub proxy_v2: bool,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub added_at: i64,
    #[serde(default)]
    pub added_by: String,
}

/// The default-route host in [`Passthrough::host`].
pub const PASSTHROUGH_DEFAULT: &str = "*";
/// Most passthrough names on one box.
pub const MAX_PASSTHROUGH: usize = 64;

pub fn passthrough_path() -> PathBuf {
    crate::paths::k2_home().join("front443-passthrough.json")
}

/// The owner's passthrough names (a missing file is none). An unreadable
/// or malformed file is an error: callers say so rather than drop routes.
pub fn load_passthrough_checked() -> Result<Vec<Passthrough>, String> {
    let p = passthrough_path();
    match std::fs::read_to_string(&p) {
        Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{}: {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("read {}: {e}", p.display())),
    }
}

pub fn save_passthrough(v: &[Passthrough]) -> Result<(), String> {
    let p = passthrough_path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("mkdir {}: {e}", d.display()))?;
    }
    let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    crate::fs_atomic::atomic_write_private(&p, text.as_bytes()).map_err(|e| format!("write {}: {e}", p.display()))
}

/// Loopback ports K2 itself uses behind or beside the front: a
/// passthrough upstream may not be one of them.
pub const RESERVED_BACKEND_PORTS: [u16; 6] = [443, 5349, 8443, crate::skin_door::LOOPBACK_PORT, CADDY_FRONT_PORT, APP_DOOR_PORT];
