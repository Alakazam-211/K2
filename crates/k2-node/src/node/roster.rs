//! The owner's node roster on the node (protocol 9, C1; vs-live V11: the
//! node is the courier).
//!
//! The owner server hands the node its signed roster and the seat
//! statements naming this node (`node_roster.set`, an operate command its
//! controller signed). The node:
//! - verifies every document with the roster's key, and pins that key the
//!   first time (later rosters under another key are refused:
//!   `node_roster_key_changed`; the owner's roster key is long-lived);
//! - keeps only a rising `seq` (a lower one is `node_roster_replayed`);
//! - takes a roster only for **its own** owner server
//!   (`owner_server_fp` = the owner pin);
//! - relays the roster, with each server's **own** seat statements only, to
//!   every protocol-9 session (`node_roster` frame): once per session and on
//!   every change. A protocol-8 server never sees the frame.
//!
//! The documents verify themselves at every reader (the client server, its
//! signer); the node's checks keep a stale or foreign roster from being
//! relayed, nothing more.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use k2_node_proto::admin::{code, AdminRefusal, ApprovalKey};
use k2_node_proto::frames::Frame;
use k2_node_proto::lanes::{codes, NodeRosterDoc, OpenedRoster};

use super::tenant::Tenant;
use super::Node;

/// In the node's state dir (node user, 0600).
pub const ROSTER_FILE: &str = "node-roster.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Held {
    /// The pinned roster key (`ed25519:<b64>`).
    key: String,
    doc: NodeRosterDoc,
}

/// Root's lanes file for a node laid out at `layout`: the installed path
/// beside `/etc/k2-node`, a test node's beside its config (as the executor
/// writes it). The node reads it through `lane_secrets::FileLanes`
/// (`Node::lane_of`: receipt v2's `lane` and the lane secret rules).
pub fn lanes_file_for(layout: &crate::paths::Layout) -> std::path::PathBuf {
    let cfg = &layout.config;
    if cfg == std::path::Path::new("/etc/k2-node") {
        std::path::PathBuf::from(k2_node_proto::lanes::LANES_FILE)
    } else {
        cfg.join("k2-node-lanes.toml")
    }
}

impl Node {
    fn roster_path(&self) -> std::path::PathBuf {
        self.layout().state().join(ROSTER_FILE)
    }

    fn roster_read(&self) -> Option<Held> {
        let raw = std::fs::read(self.roster_path()).ok()?;
        serde_json::from_slice(&raw).ok()
    }

    /// The roster this node holds, verified again (a damaged file is no roster).
    pub fn roster_held(&self) -> Option<OpenedRoster> {
        let h = self.roster_read()?;
        let key = ApprovalKey::parse(&h.key)?;
        h.doc.open(Some(&key)).ok()
    }

    /// What `t` may see: the roster and its own seats (a server never sees
    /// another client's identity). `None` before the node holds a roster,
    /// and for a protocol-8 session.
    pub fn roster_for(&self, t: &Tenant) -> Option<NodeRosterDoc> {
        if !t.speaks_ci() {
            return None;
        }
        let fp = t.pin.lock().unwrap().as_ref().map(|p| p.controller_fp.clone())?;
        Some(self.roster_held()?.for_server(&fp))
    }

    /// The highest roster `seq` this node holds (receipt v2's
    /// `roster_seq_seen`).
    pub fn roster_seq_seen(&self) -> Option<u64> {
        self.roster_held().map(|r| r.roster.seq)
    }

    /// Root's lanes file (`/etc/k2-node-lanes.toml` on an installed node; a
    /// test node's beside its config, as the executor writes it).
    pub fn lanes_file(&self) -> std::path::PathBuf {
        lanes_file_for(self.layout())
    }

    /// `node_roster.set`: verify, pin, keep, relay.
    pub fn set_roster(&self, args: &Value) -> Result<Value, AdminRefusal> {
        let doc: NodeRosterDoc = serde_json::from_value(args.clone()).map_err(|e| AdminRefusal::new(code::BAD_ARGS, e.to_string()))?;
        let held = self.roster_read();
        let pinned = match &held {
            Some(h) => Some(ApprovalKey::parse(&h.key).ok_or_else(|| AdminRefusal::new(code::FAILED, "the pinned roster key is unreadable"))?),
            None => None,
        };
        let opened = doc.open(pinned.as_ref()).map_err(|e| {
            let c = if e.starts_with(codes::ROSTER_KEY_CHANGED) { codes::ROSTER_KEY_CHANGED } else { code::BAD_ARGS };
            AdminRefusal::new(c, e)
        })?;
        let owner_fp = self.owner().pin.lock().unwrap().as_ref().map(|p| p.controller_fp.clone()).unwrap_or_default();
        if owner_fp.is_empty() || !k2_node_proto::crypto::ct_eq(opened.roster.owner_server_fp.as_bytes(), owner_fp.as_bytes()) {
            return Err(AdminRefusal::new(codes::ROSTER_WRONG_OWNER, "the roster names another owner server than this node's"));
        }
        if let Some(cur) = held.as_ref().and_then(|h| h.doc.roster.peek::<k2_node_proto::node_roster::NodeRoster>(k2_node_proto::ci::DOMAIN_NODE_ROSTER).ok()) {
            if opened.roster.seq < cur.seq {
                return Err(AdminRefusal::new(codes::ROSTER_OLDER, format!("roster seq {} is older than the {} this node holds", opened.roster.seq, cur.seq)));
            }
        }
        let me = self.key.fingerprint();
        // The node keeps only the seats naming it.
        let name = opened.name_of(&me).map(str::to_string);
        let mut keep = opened.clone();
        keep.seats.retain(|(_, st)| name.as_ref().is_some_and(|n| st.nodes.contains(n)));
        let h = Held { key: opened.key.encode(), doc: keep.doc() };
        let bytes = serde_json::to_vec(&h).map_err(|e| AdminRefusal::new(code::FAILED, e.to_string()))?;
        crate::util::atomic_write(&self.roster_path(), &bytes, 0o600).map_err(|e| AdminRefusal::new(code::FAILED, e))?;
        let mut relayed = 0;
        for t in self.tenants() {
            if let Some(d) = self.roster_for(&t) {
                t.hub.send(Frame::NodeRoster(d));
                relayed += 1;
            }
        }
        Ok(json!({"seq": opened.roster.seq, "name": name, "seats": h.doc.seats.len(), "relayed": relayed}))
    }
}
