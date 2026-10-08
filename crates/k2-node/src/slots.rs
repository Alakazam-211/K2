//! Warm target slots (§11.2): `cache/<project_key>/target-<n>`, leased
//! one job at a time, preferring the slot whose last build was the same
//! commit. `slots.json` keeps each slot's last sha across restarts.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use k2_node_proto::frames::SlotInfo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub project_key: String,
    /// `None` = job-private target (all slots busy).
    pub slot: Option<u32>,
    pub dir: PathBuf,
    /// `warm` or `cold`.
    pub warmth: &'static str,
}

#[derive(Default)]
pub struct Slots {
    busy: HashSet<(String, u32)>,
}

fn state_path(project_cache: &Path) -> PathBuf {
    project_cache.join("slots.json")
}

fn read_state(project_cache: &Path) -> BTreeMap<u32, String> {
    std::fs::read_to_string(state_path(project_cache))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

impl Slots {
    /// Lease a slot for `commit`. `job_private` is used when all are busy.
    pub fn lease(&mut self, project_cache: &Path, project_key: &str, count: u32, commit: &str, job_private: PathBuf) -> Lease {
        let state = read_state(project_cache);
        let free: Vec<u32> = (1..=count.max(1)).filter(|n| !self.busy.contains(&(project_key.to_string(), *n))).collect();
        let pick = free
            .iter()
            .copied()
            .find(|n| state.get(n).map(|s| s == commit).unwrap_or(false))
            .or_else(|| free.iter().copied().find(|n| state.contains_key(n)))
            .or_else(|| free.first().copied());
        match pick {
            Some(n) => {
                self.busy.insert((project_key.to_string(), n));
                Lease {
                    project_key: project_key.to_string(),
                    slot: Some(n),
                    dir: project_cache.join(format!("target-{n}")),
                    warmth: if state.contains_key(&n) { "warm" } else { "cold" },
                }
            }
            None => Lease { project_key: project_key.to_string(), slot: None, dir: job_private, warmth: "cold" },
        }
    }

    /// Give the slot back, recording the commit it last built.
    pub fn release(&mut self, project_cache: &Path, lease: &Lease, commit: Option<&str>) {
        let Some(n) = lease.slot else { return };
        self.busy.remove(&(lease.project_key.clone(), n));
        if let Some(c) = commit {
            let mut state = read_state(project_cache);
            state.insert(n, c.to_string());
            if let Ok(body) = serde_json::to_vec(&state) {
                let _ = crate::util::atomic_write(&state_path(project_cache), &body, 0o644);
            }
        }
    }

    /// Every known slot under `cache` for the offer.
    pub fn list(&self, cache: &Path) -> Vec<SlotInfo> {
        let mut out = Vec::new();
        let Ok(dir) = std::fs::read_dir(cache) else { return out };
        let mut projects: Vec<_> = dir.flatten().filter(|e| e.path().is_dir()).collect();
        projects.sort_by_key(|e| e.file_name());
        for e in projects {
            let key = e.file_name().to_string_lossy().into_owned();
            if key == "sccache" {
                continue;
            }
            for (n, sha) in read_state(&e.path()) {
                out.push(SlotInfo { busy: self.busy.contains(&(key.clone(), n)), project_key: key.clone(), slot: n, last_sha: Some(sha) });
            }
            for (k, n) in &self.busy {
                if *k == key && !out.iter().any(|s| s.project_key == key && s.slot == *n) {
                    out.push(SlotInfo { project_key: key.clone(), slot: *n, busy: true, last_sha: None });
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_matching_sha_then_warm_then_cold_then_private() {
        let d = crate::util::temp_dir("slots");
        let pc = d.join("pk");
        std::fs::create_dir_all(&pc).unwrap();
        let mut s = Slots::default();
        let a = s.lease(&pc, "pk", 2, "c1", d.join("priv"));
        assert_eq!((a.slot, a.warmth), (Some(1), "cold"));
        s.release(&pc, &a, Some("c1"));
        let b = s.lease(&pc, "pk", 2, "c2", d.join("priv"));
        assert_eq!((b.slot, b.warmth), (Some(1), "warm"), "a warm slot beats an empty one");
        let c = s.lease(&pc, "pk", 2, "c1", d.join("priv"));
        assert_eq!((c.slot, c.warmth), (Some(2), "cold"));
        let p = s.lease(&pc, "pk", 2, "c1", d.join("priv"));
        assert_eq!((p.slot, p.dir.clone()), (None, d.join("priv")));
        s.release(&pc, &b, Some("c2"));
        s.release(&pc, &c, Some("c1"));
        // slot 1 = c2, slot 2 = c1: c1 goes to 2 even though 1 is free first.
        let again = s.lease(&pc, "pk", 2, "c1", d.join("priv"));
        assert_eq!(again.slot, Some(2));
        let listed = s.list(&d);
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().any(|x| x.slot == 2 && x.busy && x.last_sha.as_deref() == Some("c1")));
    }
}
