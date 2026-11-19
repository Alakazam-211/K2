//! Postal's own files under the K2 home, 0600 in 0700 folders, never in a
//! table (prd-postal-in-k2-v1 §5.6, P25):
//!
//! - **`postal-outbox/<handle id>/<message id>.json`**: one outgoing
//!   message: the SDK's outbox [`postal_sdk::outbox::Item`] and what it
//!   needs to sign again on each try (same id, a fresh `sent_at`). Removed
//!   once the item is final (delivered or dead; the delivery record keeps
//!   the history).
//! - **`postal-inbound/<delivery id>.json`**: a received message between
//!   the durable store and its landing. The delivery row and this file
//!   are written before the plane is ACKed; `notify` lands it (terminal,
//!   inbox, Thread) and deletes it. A restart lands what is left.
//!
//! Bodies live here only until they land where `k2 msg` bodies live.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use super::route::Plan;

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

pub fn outbox_root() -> PathBuf {
    crate::paths::k2_home().join("postal-outbox")
}

pub fn inbound_root() -> PathBuf {
    crate::paths::k2_home().join("postal-inbound")
}

fn safe_name(s: &str) -> bool {
    (1..=80).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn write(dir: &Path, name: &str, v: &impl Serialize) -> Result<(), String> {
    if !safe_name(name) {
        return Err(format!("postal file name refused: {name}"));
    }
    crate::fs_atomic::create_private_dir_all(dir).map_err(|e| format!("postal folder: {e}"))?;
    let bytes = serde_json::to_vec(v).map_err(|e| format!("postal file: {e}"))?;
    crate::fs_atomic::atomic_write_private(&dir.join(format!("{name}.json")), &bytes).map_err(|e| format!("postal file: {e}"))
}

fn read_all<T: for<'de> Deserialize<'de>>(dir: &Path) -> Vec<(PathBuf, T)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<(PathBuf, T)> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| {
            let t = serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?;
            Some((p, t))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

// ---------------------------------------------------------------- outbox

/// One outgoing message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outgoing {
    pub item: postal_sdk::outbox::Item,
    pub handle_id: String,
    pub workspace_id: String,
    /// The sender's address (this box's handle).
    pub from: String,
    pub body: String,
    pub content_type: String,
    pub wake: bool,
    pub tray: bool,
    /// The badge of the copy the plane holds (its first write), once held.
    pub badge: Option<String>,
    /// The sender's own delivery record (`m_`), for status lookups.
    pub record_id: String,
}

pub fn outbox_dir(handle_id: &str) -> PathBuf {
    outbox_root().join(handle_id)
}

pub fn save_outgoing(o: &Outgoing) -> Result<(), String> {
    write(&outbox_dir(&o.handle_id), &o.item.id, o)
}

pub fn remove_outgoing(o: &Outgoing) {
    let _ = std::fs::remove_file(outbox_dir(&o.handle_id).join(format!("{}.json", o.item.id)));
}

/// Every queued outgoing message, all handles.
pub fn outgoing() -> Vec<Outgoing> {
    let Ok(rd) = std::fs::read_dir(outbox_root()) else { return Vec::new() };
    let mut out = Vec::new();
    for d in rd.filter_map(Result::ok).filter(|e| e.path().is_dir()) {
        out.extend(read_all::<Outgoing>(&d.path()).into_iter().map(|(_, o)| o));
    }
    out
}

pub fn outgoing_by_id(id: &str) -> Option<Outgoing> {
    outgoing().into_iter().find(|o| o.item.id == id)
}

// ---------------------------------------------------------------- inbound

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboundFile {
    pub storage_name: String,
    pub b64: String,
}

/// A received message waiting to land.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Inbound {
    pub delivery_id: String,
    pub message_id: String,
    pub workspace_id: String,
    pub to: String,
    pub from: String,
    pub badge: String,
    pub content_type: String,
    pub body_b64: String,
    #[serde(default)]
    pub files: Vec<InboundFile>,
    pub held: bool,
    pub plan: Plan,
    /// Landing tries so far (`notify`), and not before this (unix s).
    #[serde(default)]
    pub tries: u32,
    #[serde(default)]
    pub next_at: i64,
}

impl Inbound {
    pub fn body(&self) -> Vec<u8> {
        b64().decode(&self.body_b64).unwrap_or_default()
    }

    pub fn encode_body(bytes: &[u8]) -> String {
        b64().encode(bytes)
    }

    pub fn encode_file(storage_name: &str, bytes: &[u8]) -> InboundFile {
        InboundFile { storage_name: storage_name.to_string(), b64: b64().encode(bytes) }
    }

    pub fn file_bytes(f: &InboundFile) -> Vec<u8> {
        b64().decode(&f.b64).unwrap_or_default()
    }
}

pub fn save_inbound(i: &Inbound) -> Result<(), String> {
    write(&inbound_root(), &i.delivery_id, i)
}

pub fn remove_inbound(delivery_id: &str) {
    if safe_name(delivery_id) {
        let _ = std::fs::remove_file(inbound_root().join(format!("{delivery_id}.json")));
    }
}

pub fn inbound() -> Vec<Inbound> {
    read_all::<Inbound>(&inbound_root()).into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_that_could_leave_the_folder_are_refused() {
        assert!(safe_name("m_AbC123") && safe_name("01ARZ3NDEKTSV4RRFFQ69G5FAV"));
        for bad in ["", "../x", "a/b", "a.json", "a b"] {
            assert!(!safe_name(bad), "{bad}");
        }
    }
}
