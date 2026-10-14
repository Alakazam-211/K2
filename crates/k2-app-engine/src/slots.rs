//! Per-App id slots: App `x` keeps slot `n` (ids `slot_base(n)..+65536`
//! in `k2app`'s namespace, gateway port `38600 + n`) from its first
//! prepare until a purge. Stored in `<state>/slots.json` (k2app 0600),
//! written atomically under the engine's state lock.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slots {
    pub version: u32,
    pub apps: BTreeMap<String, u64>,
}

impl Slots {
    /// The App's slot, assigning the lowest free one below `max`.
    pub fn assign(&mut self, app: &str, max: u64) -> Option<u64> {
        if let Some(n) = self.apps.get(app) {
            return Some(*n);
        }
        let used: std::collections::BTreeSet<u64> = self.apps.values().copied().collect();
        let n = (0..max).find(|n| !used.contains(n))?;
        self.apps.insert(app.to_string(), n);
        self.version = 1;
        Some(n)
    }

    pub fn get(&self, app: &str) -> Option<u64> {
        self.apps.get(app).copied()
    }

    pub fn free(&mut self, app: &str) -> bool {
        self.apps.remove(app).is_some()
    }
}

pub fn load(path: &Path) -> Result<Slots, String> {
    match std::fs::read(path) {
        Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("slots.json unreadable: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Slots { version: 1, apps: BTreeMap::new() }),
        Err(e) => Err(format!("read slots.json: {e}")),
    }
}

pub fn save(path: &Path, s: &Slots) -> Result<(), String> {
    crate::sys::write_atomic(path, &serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?, 0o600)
}

/// How many 65536-id slots `k2app`'s subordinate ranges hold (from
/// `/etc/subuid` and `/etc/subgid`; the smaller wins), capped at
/// [`k2_root_proto::engine::MAX_SLOTS`].
pub fn capacity(subuid: &str, subgid: &str, user: &str) -> u64 {
    let total = |text: &str| -> u64 {
        text.lines()
            .filter_map(|l| {
                let mut it = l.trim().split(':');
                let (u, _start, n) = (it.next()?, it.next()?, it.next()?);
                (u == user).then(|| n.parse::<u64>().ok()).flatten()
            })
            .sum()
    };
    let ids = total(subuid).min(total(subgid));
    (ids / k2_root_proto::engine::SLOT_SIZE).min(k2_root_proto::engine::MAX_SLOTS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_are_stable_lowest_free_and_bounded() {
        let mut s = Slots::default();
        assert_eq!(s.assign("a", 3), Some(0));
        assert_eq!(s.assign("b", 3), Some(1));
        assert_eq!(s.assign("a", 3), Some(0), "stable");
        assert_eq!(s.assign("c", 3), Some(2));
        assert_eq!(s.assign("d", 3), None, "full");
        assert!(s.free("b"));
        assert_eq!(s.assign("d", 3), Some(1), "reuses the freed slot");
        assert!(!s.free("zz"));
    }

    #[test]
    fn capacity_from_subid_files() {
        let sub = "k2:100000:65536\nk2app:1048576:4194304\n";
        assert_eq!(capacity(sub, sub, "k2app"), 64);
        assert_eq!(capacity(sub, "k2app:1048576:131072\n", "k2app"), 2);
        assert_eq!(capacity(sub, sub, "nobody"), 0);
        assert_eq!(capacity("k2app:1:999999999999\n", "k2app:1:999999999999\n", "k2app"), 64);
    }

    #[test]
    fn file_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("slots.json");
        let mut s = load(&p).unwrap();
        s.assign("jobs", 64);
        save(&p, &s).unwrap();
        assert_eq!(load(&p).unwrap().get("jobs"), Some(0));
        std::fs::write(&p, "{\"version\":1,\"apps\":{},\"x\":1}").unwrap();
        assert!(load(&p).is_err());
    }
}
