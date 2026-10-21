//! `run/status.json` (0644): what the node machine's K2, `k2-node status`
//! and the owner read. Written by `k2-node` only; display, not authority.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub version: String,
    pub protocol: u32,
    pub node_fp: String,
    pub name: Option<String>,
    pub node_id: Option<String>,
    pub controller: Option<String>,
    pub controller_fp: Option<String>,
    /// unenrolled | pending | connecting | online | offline | revoked
    pub state: String,
    pub route: Option<String>,
    pub since: i64,
    pub last_error: Option<String>,
    pub running: Vec<String>,
    pub control: String,
    pub control_by: Option<String>,
    pub policy_error: Option<String>,
    pub unavailable: Vec<String>,
    pub holding_lock: Option<String>,
    pub stale_locks: Vec<String>,
    pub caps_hard: bool,
    /// The job wall: `job_user` | `seatbelt` | `none` (0.46.3).
    #[serde(default)]
    pub wall: String,
    /// The node enforces `network = "none"` (the machine's K2 lets the
    /// owner set it only then).
    #[serde(default)]
    pub network_none: bool,
    /// Shown while pending.
    pub sas: Option<String>,
    pub notes: Vec<String>,
    pub updated_at: i64,
    /// The other servers this node serves (prd-compute-shared-queue-v1;
    /// empty in solo mode). The owner server is the fields above.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<ServerLine>,
    /// `servers.toml` is unreadable or invalid: the node serves on with the
    /// last good copy (or solo), never "fail open" (§14).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub servers_error: Option<String>,
}

/// One non-owner server as the node sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerLine {
    pub slot: u8,
    pub label: String,
    /// The share's status (`pending`, `active`, `paused`, `expired`,
    /// `revoked`, `refused`).
    pub state: String,
    pub trust: String,
    /// The connection: `unenrolled`, `enrolling`, `pending`, `connecting`,
    /// `online`, `offline`, `refused`, `revoked`, `stopped`.
    pub link: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// The SAS to read to the new server's owner, while it's unconfirmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sas: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fp: Option<String>,
    #[serde(default)]
    pub running: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

pub fn write(path: &std::path::Path, s: &Status) -> Result<(), String> {
    let body = serde_json::to_vec_pretty(s).map_err(|e| format!("encode status: {e}"))?;
    crate::util::atomic_write(path, &body, 0o644)
}

pub fn read(path: &std::path::Path) -> Result<Status, String> {
    let s = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_str(&s).map_err(|e| format!("{}: {e}", path.display()))
}
