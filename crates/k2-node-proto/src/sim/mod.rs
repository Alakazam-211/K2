//! iPhone on a Mac compute node (protocol 11, `prd-ios-on-mac-node-0467-v1`,
//! review IR-1…IR-17): the wire between a server's `ios-node` sim lane and
//! a Mac node's Simulator service.
//!
//! A **sim session** is a new long-lived kind, not a batch job: no argv,
//! no artifacts, no receipt, and a lease kill that batch jobs never have.
//!
//! - On the **attach socket** (signed envelopes, [`crate::frames`]):
//!   [`SimOpen`], [`SimClose`], [`SimErase`], [`SimSync`] go server → node;
//!   [`SimStateFrame`] goes node → server; [`SimFacts`] rides in
//!   `Offer.sim`. Only to and from peers at protocol ≥ [`PROTOCOL_SIM`]
//!   whose offer lists [`FEATURE_SIM_IOS`].
//! - The **stream socket** (`/cli/compute/sim-stream`, one per session,
//!   dialed by k2-node): [`stream`], an X25519 handshake signed by both
//!   long-term keys, then AES-256-GCM per direction with keys bound to that
//!   one connection (IR-1, IR-2). The relay sees sizes only.
//! - Inside the stream: channel 0 carries [`ctl`] JSON; channels 1…n carry
//!   one Simulator helper each.
//! - [`policy`]: what the node runs for a server, argument by argument
//!   (IR-5). [`pack`]: the `.app` upload rules (IR-3). [`lease`]: the
//!   node's session table (grace, expiry, reconcile; IR-6, IR-7, IR-8).
//!
//! No floats anywhere (canonical JSON): a screen scale is in thousandths.

pub mod ctl;
pub mod lease;
pub mod pack;
pub mod policy;
pub mod stream;

use serde::{Deserialize, Serialize};

/// The first protocol with sim sessions.
pub const PROTOCOL_SIM: u32 = 11;
/// The offer feature: this node runs iPhones for this server's slot.
pub const FEATURE_SIM_IOS: &str = "sim-ios";
/// The stream socket's path (through the same Connect URL as attach).
pub const STREAM_PATH: &str = "/cli/compute/sim-stream";

/// The Swift helper's source: one copy, built by the node with its own
/// Xcode (k2-node never links k2-core, IN-A10) and by a Mac daemon's local
/// `ios` lane. The server never ships code to a node.
pub const HELPER_SOURCE: &str = include_str!("k2-simhelper.swift");
/// The helper's newest wire protocol (its `ready`/`hello` report it, with
/// the ones it `accepts`). The same number as the daemon's
/// `k2_simvideo::link::PROTOCOL_VERSION` (a k2-core test pins them). A
/// node whose helper speaks an older one is `sim_node_outdated`.
pub const HELPER_PROTOCOL: u64 = 4;

/// Default and maximum lease (§3.2, Q8): sessions end early when idle.
pub const LEASE_MAX_SECS: u64 = 8 * 3600;
/// How long a node keeps a device while its stream is down (§3.2).
pub const GRACE_SECS: u64 = 120;
/// Memory a session reserves under the node's cap (§3.2; S0 measures it).
pub const RESERVE_MEM_BYTES: u64 = 3 << 30;
/// Above reservation + this, simhost ends the session (`sim_mem_cap`, IR-8).
pub const MEM_SLACK_BYTES: u64 = 1 << 30;
/// CPU a session reserves, in thousandths of a core (IR-8).
pub const RESERVE_MILLICPU: u32 = 1500;
/// One session per node in v1 (Q10).
pub const MAX_SESSIONS_PER_NODE: usize = 1;
/// Frame rate cap for this lane and the default for remote viewers (§3.4).
pub const FPS_MAX: u32 = 15;
pub const FPS_DEFAULT: u32 = 8;

/// Codes both ends use. Session states carry them as `code`.
pub mod codes {
    /// The node refused a command or argument (policy, IR-5).
    pub const NODE_REFUSED: &str = "sim_node_refused";
    /// The node already runs a session, is held, draining, paused, or low
    /// on memory or disk.
    pub const NODE_BUSY: &str = "sim_node_busy";
    /// The node's helper or protocol is older than the server's.
    pub const NODE_OUTDATED: &str = "sim_node_outdated";
    /// This server's slot hasn't turned sim on (`k2-node servers set
    /// <slot> --sim on`), or the node isn't set up for it.
    pub const NOT_ENABLED: &str = "sim_not_enabled";
    /// The stream stayed down past the grace.
    pub const LEASE_LOST: &str = "lease_lost";
    /// The lease's hard end.
    pub const LEASE_EXPIRED: &str = "lease_expired";
    /// The Simulator used more than reservation + slack (IR-8).
    pub const MEM_CAP: &str = "sim_mem_cap";
    /// The Simulator service exited.
    pub const SIMHOST_EXITED: &str = "simhost_exited";
    /// The node rebooted or k2-node restarted (IR-7).
    pub const NODE_RESTARTED: &str = "node_restarted";
    /// The server no longer holds the session (`sim_sync`, IR-7).
    pub const NOT_HELD: &str = "not_held";
    /// The same workspace opened a new session in grace (IR-7).
    pub const REPLACED: &str = "replaced";
    /// The server asked (`sim_close`).
    pub const CLOSED: &str = "closed";
    /// Malformed ids or sizes in a sim frame.
    pub const BAD_FRAME: &str = "sim_bad_frame";
}

/// A sim session id: 32 lowercase hex (the server mints it).
pub fn valid_session(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// A workspace key: 24 lowercase hex (`k2_core::sim::workspace_key`), never
/// a handle (handles can be renamed, and they name people's work).
pub fn valid_workspace_key(s: &str) -> bool {
    s.len() == 24 && s.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// A Simulator UDID: 8-4-4-4-12 upper or lower hex.
pub fn valid_udid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts.iter().zip([8usize, 4, 4, 4, 12]).all(|(p, n)| p.len() == n && p.bytes().all(|c| c.is_ascii_hexdigit()))
}

// ── attach frames ────────────────────────────────────────────────────

/// Server → node: lease a device session for one workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimOpen {
    pub session: String,
    pub workspace_key: String,
    pub mem_bytes: u64,
    pub lease_secs: u64,
}

impl SimOpen {
    /// Ids well formed, sizes in range.
    pub fn valid(&self) -> bool {
        valid_session(&self.session)
            && valid_workspace_key(&self.workspace_key)
            && (1..=LEASE_MAX_SECS).contains(&self.lease_secs)
            && self.mem_bytes >= 1 << 30
            && self.mem_bytes <= 64 << 30
    }
}

/// Server → node: end a session (the device shuts down; its data stays).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimClose {
    pub session: String,
    pub reason: String,
}

/// Server → node: delete a workspace's devices and uploads on this node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimErase {
    pub workspace_key: String,
}

/// Server → node, right after every attach (IR-7): the sessions this
/// server still holds on this node. The node closes any other session of
/// this server's slot at once (`not_held`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimSync {
    pub sessions: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Admitted; the node is dialing the stream.
    Opening,
    /// The stream is up.
    Open,
    /// The stream dropped; the node keeps the device for [`GRACE_SECS`].
    Reconnecting,
    Closed,
    Refused,
}

impl SessionState {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionState::Opening => "opening",
            SessionState::Open => "open",
            SessionState::Reconnecting => "reconnecting",
            SessionState::Closed => "closed",
            SessionState::Refused => "refused",
        }
    }
    pub fn is_final(self) -> bool {
        matches!(self, SessionState::Closed | SessionState::Refused)
    }
}

/// Node → server: a session changed state. `code` is one of [`codes`]
/// for `closed` / `refused`; `message` is for a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimStateFrame {
    pub session: String,
    pub state: SessionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

// ── the node's facts (Offer.sim) ─────────────────────────────────────

/// One step the node owner still has to take (`xcodebuild -downloadPlatform iOS`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimNeed {
    pub what: String,
    pub command: String,
}

/// A device type the node can boot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimDeviceType {
    /// `com.apple.CoreSimulator.SimDeviceType.iPhone-17`.
    pub identifier: String,
    pub name: String,
    pub width_pt: u32,
    pub height_pt: u32,
    /// Pixels per point × 1000 (no floats on the wire).
    pub scale_milli: u32,
    /// The plainest newest iPhone: booted when nobody names one.
    #[serde(default)]
    pub default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimRuntime {
    pub identifier: String,
    /// `iOS 26.5`.
    pub name: String,
    pub version: String,
    /// iPhones only (the lane is portrait iPhone only, §7).
    pub device_types: Vec<SimDeviceType>,
}

/// What a Mac node offers a server whose slot turned sim on. Absent for
/// every other server, and below protocol 11.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SimFacts {
    /// Xcode, runtime, helper and service all ready.
    pub ready: bool,
    /// Owner steps still needed (exact commands).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<SimNeed>,
    /// `xcodebuild -version` → `26.6 (17F113)` (not the CLT, IN-A7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xcode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xcode_path: Option<String>,
    /// The helper protocol this node builds ([`HELPER_PROTOCOL`]).
    pub helper_protocol: u64,
    /// Newest first.
    #[serde(default)]
    pub runtimes: Vec<SimRuntime>,
    /// This server's live sessions on the node (IR-7: a row whose session
    /// is missing is `node_restarted`).
    #[serde(default)]
    pub sessions: Vec<String>,
    /// Sessions of every server on the node (for "busy").
    #[serde(default)]
    pub sessions_total: u32,
    /// Bytes under the Simulator user's home (IR-15).
    #[serde(default)]
    pub bytes: u64,
    /// Memory a session reserves on this node.
    pub reserve_bytes: u64,
    /// Free memory under the node's cap for a new session right now.
    pub free_bytes: u64,
    /// This server's paths on the node. The server passes them back
    /// verbatim; the node polices every one ([`policy`]).
    pub paths: policy::SlotPaths,
    /// How the node owner lets K2 update k2-node from a server: `true`
    /// when remote updates are allowed (IR-13 picks the teaching line).
    #[serde(default)]
    pub remote_update: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_checked() {
        assert!(valid_session(&"a1".repeat(16)));
        assert!(!valid_session(&"A1".repeat(16)), "lowercase only");
        assert!(!valid_session("../etc"));
        assert!(valid_workspace_key(&"0f".repeat(12)));
        assert!(!valid_workspace_key("web"), "never a handle");
        assert!(valid_udid("AD6DB1AF-E89F-48D1-A6D7-DDEF9C68B1EB"));
        assert!(!valid_udid("AD6DB1AF-E89F-48D1-A6D7-DDEF9C68B1E"));
        assert!(!valid_udid("booted"));
    }

    #[test]
    fn open_frames_are_bounded() {
        let ok = SimOpen { session: "a".repeat(32), workspace_key: "b".repeat(24), mem_bytes: RESERVE_MEM_BYTES, lease_secs: LEASE_MAX_SECS };
        assert!(ok.valid());
        assert!(!SimOpen { lease_secs: LEASE_MAX_SECS + 1, ..ok.clone() }.valid());
        assert!(!SimOpen { lease_secs: 0, ..ok.clone() }.valid());
        assert!(!SimOpen { mem_bytes: 1, ..ok.clone() }.valid());
        assert!(!SimOpen { workspace_key: "web".into(), ..ok }.valid());
    }

    #[test]
    fn the_helper_source_is_here_and_speaks_its_protocol() {
        assert!(HELPER_SOURCE.contains(&format!("let HELPER_VERSION = {HELPER_PROTOCOL}")));
    }
}
