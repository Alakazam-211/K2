//! The server-confirmed tunnel label (K2 Web's relay name binding, 0.46.8).
//!
//! The relay will accept only exact proxy names, `k2so-<label>` (the main
//! frpc) and `k2voice-<label>` (the voice lane), where `<label>` is the
//! token's label as the control plane confirms it. `tunnel.json`'s
//! `subdomain` is the REQUESTED label: advisory input that can be stale
//! (`rosson` after a move), odd (`rosson.k2.dev`, `staging.rosson`, `Rosson`)
//! or empty. It still goes to the relay as the requested `subdomain`, but
//! it never names a proxy.
//!
//! - **Source:** `GET /subdomains` with the tunnel token; the one row with
//!   `primary: true`, which k2-connect sets on the parent subdomain it
//!   resolved from that token (`subdomains::confirmed_of`). Nothing else
//!   counts: no guess from the requested label or from row order.
//! - **Cache:** `~/.k2/tunnel-label.json` (0600) holds the last confirmation
//!   per token (a hash of it, never the token), so a restart or a box whose
//!   control plane is unreachable at boot keeps its name. Every `/subdomains`
//!   fetch (the routing-map refresh) renews it.
//! - **None:** no confirmed label → no tunnel proxy is rendered (never
//!   `k2so-daemon`), the reason is logged, and the connector keeps asking
//!   the control plane until it answers ([`super::connector`]).
//! - **Off switch:** `K2_TUNNEL_PROXY_NAME=config` in the daemon's
//!   environment brings back the pre-0.46.8 naming from `subdomain` (the old
//!   path, kept until the confirmed name is proven; Rosson's keep-the-old-
//!   path rule). Remove it in the release after the relay enforces names.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::config::TunnelConfig;

/// Env switch: `config` = the pre-0.46.8 proxy naming from `subdomain`.
pub const PROXY_NAME_ENV: &str = "K2_TUNNEL_PROXY_NAME";

/// The old naming is forced on (`K2_TUNNEL_PROXY_NAME=config`).
pub fn config_naming() -> bool {
    std::env::var(PROXY_NAME_ENV).is_ok_and(|v| v.trim().eq_ignore_ascii_case("config"))
}

/// One k2.dev label: 1–63 of `[a-z0-9-]`, no leading or trailing hyphen.
/// Exactly what the relay's names allow; a confirmed label that isn't one
/// is refused (it would never match).
pub fn label_ok(l: &str) -> bool {
    !l.is_empty()
        && l.len() <= 63
        && !l.starts_with('-')
        && !l.ends_with('-')
        && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `~/.k2/tunnel-label.json`.
pub fn cache_path() -> PathBuf {
    crate::paths::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".k2").join("tunnel-label.json")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    /// The first 16 hex of sha256(token): ties the label to the token
    /// without storing it.
    token_key: String,
    label: String,
    confirmed_at: i64,
}

fn token_key(token: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.trim().as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// The label the control plane last confirmed for `token` (from the cache).
pub fn cached(token: &str) -> Option<String> {
    if token.trim().is_empty() {
        return None;
    }
    let raw = std::fs::read(cache_path()).ok()?;
    let rec: Record = serde_json::from_slice(&raw).ok()?;
    (rec.token_key == token_key(token) && label_ok(&rec.label)).then_some(rec.label)
}

/// Record a confirmation for `token` (best effort; logged on failure).
pub fn remember(token: &str, label: &str) {
    if token.trim().is_empty() || !label_ok(label) || cached(token).as_deref() == Some(label) {
        return;
    }
    let rec = Record {
        token_key: token_key(token),
        label: label.to_string(),
        confirmed_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    };
    let path = cache_path();
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let res = serde_json::to_vec(&rec)
        .map_err(|e| e.to_string())
        .and_then(|b| crate::fs_atomic::atomic_write_mode(&path, &b, 0o600).map_err(|e| e.to_string()));
    match res {
        Ok(()) => crate::log_debug!("[tunnel] the control plane confirms this tunnel's label: {label}"),
        Err(e) => crate::log_debug!("[tunnel] WARN: could not record the confirmed label in {}: {e}", path.display()),
    }
}

/// The confirmed label for `cfg`'s token: the cache, else one control-plane
/// fetch (recorded). `Err` says why there is none.
pub fn resolve(cfg: &TunnelConfig) -> Result<String, String> {
    if let Some(l) = cached(&cfg.token) {
        return Ok(l);
    }
    #[cfg(test)]
    {
        if let Some(f) = *TEST_SOURCE.lock().unwrap_or_else(|p| p.into_inner()) {
            let l = f(cfg)?;
            remember(&cfg.token, &l);
            return Ok(l);
        }
    }
    let l = super::subdomains::fetch_confirmed_label(&cfg.token)?
        .ok_or_else(|| "the control plane marks no subdomain as this tunnel token's own".to_string())?;
    if !label_ok(&l) {
        return Err(format!("the control plane confirmed {l:?}, which is not a k2.dev label"));
    }
    remember(&cfg.token, &l);
    Ok(l)
}

/// Tests: what the fake control plane confirms (default: as if it confirmed
/// the requested label, lowercased, when that is one valid label).
#[cfg(test)]
pub(crate) static TEST_SOURCE: std::sync::Mutex<Option<fn(&TunnelConfig) -> Result<String, String>>> =
    std::sync::Mutex::new(None);

#[cfg(test)]
pub(crate) fn confirms_requested(cfg: &TunnelConfig) -> Result<String, String> {
    let l = cfg.subdomain.trim().to_ascii_lowercase();
    if label_ok(&l) {
        Ok(l)
    } else {
        Err(format!("the fake control plane confirms no label for {:?}", cfg.subdomain))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::test_support::with_temp_home;

    #[test]
    fn labels_are_single_lowercase_k2dev_labels() {
        for ok in ["rosson", "k2n1-m4", "a", "x9"] {
            assert!(label_ok(ok), "{ok}");
        }
        for bad in ["", "Rosson", "rosson.k2.dev", "staging.rosson", "-a", "a-", "a_b", &"a".repeat(64)] {
            assert!(!label_ok(bad), "{bad}");
        }
    }

    #[test]
    fn the_cache_is_per_token_and_never_holds_the_token() {
        with_temp_home(|| {
            assert_eq!(cached("tok-a"), None);
            remember("tok-a", "rosson");
            assert_eq!(cached("tok-a").as_deref(), Some("rosson"));
            assert_eq!(cached("tok-b"), None, "another token has no confirmation");
            let raw = std::fs::read_to_string(cache_path()).unwrap();
            assert!(!raw.contains("tok-a"), "{raw}");
            remember("tok-a", "Bad.Label");
            assert_eq!(cached("tok-a").as_deref(), Some("rosson"), "an invalid label is never recorded");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(std::fs::metadata(cache_path()).unwrap().permissions().mode() & 0o777, 0o600);
            }
        });
    }
}
