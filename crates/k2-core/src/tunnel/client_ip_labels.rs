//! Nested services that opted in to K2's client-IP headers (0.46.3).
//!
//! Where the E2E listener splices a nested `<label>.<sub>.k2.dev` to its
//! internal target, the default stays what it was before 0.46.3: a raw
//! byte splice. Raw TCP, h2c and custom protocols keep working. A label
//! listed here gets the HTTP/1.1 rewrite instead (k2-daemon `client_ip`):
//! client copies of the forwarding headers are dropped, and K2's
//! `X-K2-Client-IP` is added. Apps don't use this list. They get the
//! rewrite unless their manifest says `host.clientIp: false`.
//!
//! The list is daemon-local, in `~/.k2/tunnel-client-ip.json`
//! (`{"labels": [...]}`). The routing map stays with the control plane.
//! It is written by `POST /cli/publish/client-ip` (`k2 publish client-ip`)
//! and read on each nested connection, so a change applies to the next
//! connection. A missing or unreadable file means no opt-ins (raw).

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    labels: BTreeSet<String>,
}

/// `~/.k2/tunnel-client-ip.json` (honours `$HOME`, like the other tunnel files).
pub fn path() -> PathBuf {
    crate::paths::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".k2").join("tunnel-client-ip.json")
}

/// A nested label: lowercase letters, digits and inner `-`, at most 63.
pub fn normalize_label(raw: &str) -> Result<String, String> {
    let l = raw.trim().to_ascii_lowercase();
    let ok = !l.is_empty()
        && l.len() <= 63
        && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !l.starts_with('-')
        && !l.ends_with('-');
    if ok {
        Ok(l)
    } else {
        Err(format!("not a nested label: {raw:?} (letters, digits and inner '-', at most 63)"))
    }
}

/// The labels that opted in. Missing / unreadable file → none.
pub fn load() -> BTreeSet<String> {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|t| serde_json::from_str::<File>(&t).ok())
        .map(|f| f.labels)
        .unwrap_or_default()
}

/// Does `label` get the rewrite?
pub fn enabled(label: &str) -> bool {
    load().contains(&label.to_ascii_lowercase())
}

/// Turn `label` on or off. Returns the new list.
pub fn set(label: &str, on: bool) -> Result<BTreeSet<String>, String> {
    let label = normalize_label(label)?;
    let mut labels = load();
    if on {
        labels.insert(label);
    } else {
        labels.remove(&label);
    }
    let text = serde_json::to_string_pretty(&File { labels: labels.clone() }).map_err(|e| e.to_string())?;
    crate::fs_atomic::atomic_write_str(&path(), &text).map_err(|e| format!("write {}: {e}", path().display()))?;
    Ok(labels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_validate() {
        assert_eq!(normalize_label(" Shop-2 ").unwrap(), "shop-2");
        for bad in ["", "-a", "a-", "a.b", "a_b", &"a".repeat(64)] {
            assert!(normalize_label(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn set_and_load_round_trip_and_default_is_off() {
        let _home = crate::test_env::TempHome::new();
        assert!(load().is_empty(), "no file: nothing opted in");
        assert!(!enabled("svc"));
        set("svc", true).unwrap();
        set("Other", true).unwrap();
        assert!(enabled("svc") && enabled("other"));
        let left = set("svc", false).unwrap();
        assert_eq!(left.into_iter().collect::<Vec<_>>(), vec!["other".to_string()]);
        assert!(!enabled("svc"));
        assert!(set("bad label", true).is_err());
    }
}
