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
//! - **Who uses it, by default:** the voice lane only. The main tunnel's
//!   proxies keep today's names from `subdomain` (`K2_TUNNEL_PROXY_NAME`
//!   unset, or `config`): with confirmed names, a box whose label the control
//!   plane doesn't confirm would log in and serve nothing, and that hasn't
//!   been tried against a real token yet. Keep-the-old-path rule: the flip
//!   comes after a live check on the voice test boxes.
//! - **Opt-in:** `K2_TUNNEL_PROXY_NAME=confirmed` in the daemon's environment
//!   names the main tunnel's proxies from the confirmed label too. Then:
//!   no confirmed label → no tunnel proxy is rendered (never `k2so-daemon`),
//!   the reason is logged, and the connector keeps asking the control plane
//!   until it answers ([`super::connector`]).
//! - **The voice lane** always wants the confirmed label, whichever way the
//!   main tunnel is named: [`voice_label`] (no network), [`voice_label_or_ask`]
//!   (armed, preview on: asks at most every [`VOICE_ASK_EVERY`]).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::config::TunnelConfig;

/// How the main tunnel's proxies are named: `confirmed` = from the
/// server-confirmed label (opt-in); anything else, or unset, = today's naming
/// from `subdomain`.
pub const PROXY_NAME_ENV: &str = "K2_TUNNEL_PROXY_NAME";
/// The opt-in value of [`PROXY_NAME_ENV`].
pub const PROXY_NAME_CONFIRMED: &str = "confirmed";
/// How often an armed voice lane asks the control plane for a label it
/// doesn't have.
pub const VOICE_ASK_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// The main tunnel is named from `subdomain`, as before 0.46.8: the default.
/// Only `K2_TUNNEL_PROXY_NAME=confirmed` turns it off, so a typo keeps today's
/// names and never a box that serves nothing.
pub fn config_naming() -> bool {
    !std::env::var(PROXY_NAME_ENV).is_ok_and(|v| v.trim().eq_ignore_ascii_case(PROXY_NAME_CONFIRMED))
}

/// The label the voice lane uses for `cfg` without asking anyone: the one
/// the connector resolved, else the cache (every `/subdomains` fetch renews
/// it). Never the requested `subdomain`: the relay takes only the exact
/// `k2voice-<label>` of the confirmed label.
pub fn voice_label(cfg: &TunnelConfig) -> Option<String> {
    cfg.confirmed_label
        .as_deref()
        .map(str::trim)
        .filter(|l| label_ok(l))
        .map(str::to_string)
        .or_else(|| cached(&cfg.token))
}

/// [`voice_label`], asking the control plane when there's none, at most
/// every [`VOICE_ASK_EVERY`]. Only for a voice lane that is armed with the
/// preview on, and the tunnel tier's own commands: with the preview off,
/// this never runs and a box makes no extra call.
pub fn voice_label_or_ask(cfg: &TunnelConfig) -> Option<String> {
    if let Some(l) = voice_label(cfg) {
        return Some(l);
    }
    static LAST_ASK: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    {
        let mut last = LAST_ASK.lock().unwrap_or_else(|p| p.into_inner());
        if !cfg!(test) && last.is_some_and(|t| t.elapsed() < VOICE_ASK_EVERY) {
            return None;
        }
        *last = Some(std::time::Instant::now());
    }
    match resolve(cfg) {
        Ok(l) => Some(l),
        Err(why) => {
            crate::log_debug!("[tunnel] the voice lane has no confirmed label yet ({why}); asking again in {VOICE_ASK_EVERY:?}");
            None
        }
    }
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

    /// Today's naming is the default; only the exact opt-in leaves it, so
    /// a typo can't leave a box serving nothing.
    #[test]
    fn the_main_tunnel_keeps_todays_naming_unless_confirmed_is_asked_for() {
        {
            let _unset = crate::test_env::EnvVar::remove(PROXY_NAME_ENV);
            assert!(config_naming(), "unset");
        }
        for v in ["config", "CONFIG", "", "confirm", "true"] {
            let _v = crate::test_env::EnvVar::set(PROXY_NAME_ENV, v);
            assert!(config_naming(), "{v:?}");
        }
        for v in ["confirmed", " Confirmed "] {
            let _v = crate::test_env::EnvVar::set(PROXY_NAME_ENV, v);
            assert!(!config_naming(), "{v:?}");
        }
    }

    /// The voice lane's label: the resolved one, else the cache, never the
    /// requested `subdomain`; asking fills the cache and a refusal is `None`.
    #[test]
    fn the_voice_label_is_always_a_confirmed_one() {
        with_temp_home(|| {
            let mut cfg = TunnelConfig { token: "tok-v".into(), subdomain: "staging.rosson".into(), ..Default::default() };
            assert_eq!(voice_label(&cfg), None, "the request never counts");
            *TEST_SOURCE.lock().unwrap() = Some(|_| Err("the fake control plane is unreachable".into()));
            assert_eq!(voice_label_or_ask(&cfg), None);
            *TEST_SOURCE.lock().unwrap() = Some(|_| Ok("rosson".into()));
            assert_eq!(voice_label_or_ask(&cfg).as_deref(), Some("rosson"));
            assert_eq!(voice_label(&cfg).as_deref(), Some("rosson"), "from the cache now");
            cfg.confirmed_label = Some("box2".into());
            assert_eq!(voice_label(&cfg).as_deref(), Some("box2"), "what the connector resolved comes first");
            cfg.confirmed_label = Some("Not.A.Label".into());
            assert_eq!(voice_label(&cfg).as_deref(), Some("rosson"));
        });
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
