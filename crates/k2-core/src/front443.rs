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
