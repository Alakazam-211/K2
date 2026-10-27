//! Licences the box owner accepted (M-A23): `~/.k2/sim/licences.json`.
//! Only the owner accepts, on a card or in Settings; never an agent. An
//! acceptance binds to the licence text's sha1, so a changed licence needs
//! a new acceptance.

use serde::{Deserialize, Serialize};

use super::lane::LaneKind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Accepted {
    pub lane: LaneKind,
    pub id: String,
    pub sha1: String,
    /// `owner` or `login:<user>` (an Owner login).
    pub by: String,
    pub at: i64,
}

pub fn path() -> std::path::PathBuf {
    super::sim_home().join("licences.json")
}

pub fn load() -> Vec<Accepted> {
    std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// The acceptance of `id` on `lane` for exactly this text (`sha1`).
pub fn accepted(lane: LaneKind, id: &str, sha1: &str) -> Option<Accepted> {
    load().into_iter().find(|a| a.lane == lane && a.id == id && a.sha1 == sha1)
}

/// Every licence id accepted on `lane` (any text version).
pub fn accepted_ids(lane: LaneKind) -> Vec<String> {
    let mut v: Vec<String> = load().into_iter().filter(|a| a.lane == lane).map(|a| a.id).collect();
    v.sort();
    v.dedup();
    v
}

/// Record an acceptance (replacing an older one for the same lane + id).
pub fn record(a: Accepted) -> Result<(), String> {
    let mut all = load();
    all.retain(|x| !(x.lane == a.lane && x.id == a.id));
    all.push(a);
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("mkdir {}: {e}", d.display()))?;
    }
    let body = serde_json::to_vec_pretty(&all).map_err(|e| e.to_string())?;
    crate::fs_atomic::atomic_write_private(&p, &body).map_err(|e| format!("write {}: {e}", p.display()))
}

/// sha1 hex of a licence text, trimmed (sdkmanager's own hash).
pub fn text_sha1(text: &str) -> String {
    use sha1::{Digest, Sha1};
    let d = Sha1::digest(text.trim().as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}
