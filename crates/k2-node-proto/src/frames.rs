//! Frames between a controller daemon and `k2-node` (§6.2, CN2).
//!
//! Two sockets, both WebSockets carrying JSON text messages:
//!
//! - `/cli/compute/enroll`: [`HandshakeFrame::EnrollRequest`] →
//!   `EnrollChallenge` → `EnrollProof` → `EnrollDone` (or `Refused`).
//! - `/cli/compute/attach`: `Hello` → `Challenge` → `Proof`, then every
//!   frame is an [`Envelope`] around a [`Frame`], signed by the sender's
//!   key over the session id and a per-direction sequence number.
//!
//! **Why every frame is signed.** With E2E on (the default) TLS ends in
//! the controller daemon; with E2E off it ends on the relay's Caddy. A
//! signature per frame means nothing on the path (relay included) can
//! inject an `assign` into a node or a fake result into the controller,
//! and sequence numbers stop replay and reordering inside a session.
//!
//! No floats anywhere: canonical JSON of integers and strings is stable.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::crypto::{self, SigningKey};

/// Wire protocol version, compared on every `hello` (CN26).
pub const PROTOCOL: u32 = 1;

/// Largest JSON text message on either socket.
pub const MAX_FRAME_BYTES: usize = 1 << 20;
/// Largest raw log payload per `log` frame.
pub const MAX_LOG_CHUNK: usize = 64 * 1024;
/// Largest raw payload per `transfer` frame (base64 grows it by a third).
pub const TRANSFER_CHUNK: usize = 256 * 1024;
/// WebSocket ping cadence (the Thread-sync lesson: quiet sockets get cut).
pub const PING_SECS: u64 = 20;
/// Offer cadence when nothing changed.
pub const OFFER_SECS: u64 = 30;
/// Deadline for the whole attach or enroll handshake.
pub const HANDSHAKE_SECS: u64 = 5;

pub const DOMAIN_ATTACH_CONTROLLER: &str = "k2-compute-attach-v1/controller";
pub const DOMAIN_ATTACH_NODE: &str = "k2-compute-attach-v1/node";
pub const DOMAIN_SESSION: &str = "k2-compute-session-v1";
pub const DOMAIN_FRAME_C2N: &str = "k2-compute-frame-v1/c2n";
pub const DOMAIN_FRAME_N2C: &str = "k2-compute-frame-v1/n2c";
pub const DOMAIN_ENROLL_CONTROLLER: &str = "k2-compute-enroll-v1/controller";
pub const DOMAIN_ENROLL_NODE: &str = "k2-compute-enroll-v1/node";
pub const DOMAIN_RECEIPT: &str = "k2-compute-receipt-v1";

// ── handshake and enroll ─────────────────────────────────────────────

/// Unsigned frames before a session exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum HandshakeFrame {
    Hello(Hello),
    Challenge(Challenge),
    Proof(Proof),
    EnrollRequest(EnrollRequest),
    EnrollChallenge(EnrollChallenge),
    EnrollProof(EnrollProof),
    EnrollDone(EnrollDone),
    /// Either side, then close. `code` is a stable reason code.
    Refused(Refused),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub node_fp: String,
    pub protocol: u32,
    pub boot_id: String,
    pub ledger_id: String,
    /// 32 random bytes, base64.
    pub nonce_n: String,
    pub node_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Challenge {
    pub controller_fp: String,
    pub nonce_c: String,
    /// `sign_c(DOMAIN_ATTACH_CONTROLLER, [nonce_n, node_fp, nonce_c])`.
    pub sig: String,
    pub protocol: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proof {
    /// `sign_n(DOMAIN_ATTACH_NODE, [nonce_c, controller_fp, nonce_n])`.
    pub sig: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refused {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrollRequest {
    /// [`crate::pairing::code_binding`] of the code; the code itself never
    /// travels.
    pub code_binding: String,
    pub node_public_key_pem: String,
    pub name: String,
    pub labels: BTreeMap<String, String>,
    pub protocol: u32,
    pub node_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrollChallenge {
    pub controller_public_key_pem: String,
    /// 16 random bytes, base64, picked after the node's key arrived.
    pub enroll_nonce: String,
    pub node_id: String,
    /// `sign_c(DOMAIN_ENROLL_CONTROLLER, [node_fp, enroll_nonce, code_binding, controller_fp])`.
    pub sig: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrollProof {
    /// `sign_n(DOMAIN_ENROLL_NODE, [controller_fp, enroll_nonce, code_binding, node_fp])`.
    pub sig: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrollDone {
    pub node_id: String,
    pub name: String,
    /// Always `pending`: a human confirms the SAS on the controller.
    pub state: String,
}

/// The session id both ends derive after the handshake.
pub fn session_id(nonce_n: &str, nonce_c: &str) -> String {
    crypto::sha256_hex(&crypto::signed_bytes(DOMAIN_SESSION, &[nonce_n.as_bytes(), nonce_c.as_bytes()]))
}

// ── the signed envelope ──────────────────────────────────────────────

/// Which way a frame travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    ControllerToNode,
    NodeToController,
}

impl Direction {
    fn domain(self) -> &'static str {
        match self {
            Direction::ControllerToNode => DOMAIN_FRAME_C2N,
            Direction::NodeToController => DOMAIN_FRAME_N2C,
        }
    }
}

/// `{ s: seq, f: frame, g: sig }`. The signature covers the canonical
/// JSON of `f` exactly as received (verification never re-serializes a
/// typed struct, so optional fields can't break it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub s: u64,
    pub f: Value,
    pub g: String,
}

/// Seal `frame` as sequence `seq` of `session` in direction `dir`.
pub fn seal(
    key: &SigningKey,
    dir: Direction,
    session: &str,
    seq: u64,
    frame: &Frame,
) -> Result<String, String> {
    if frame.direction() != dir {
        return Err(format!("frame {} can't travel this way", frame.kind()));
    }
    let f = serde_json::to_value(frame).map_err(|e| format!("encode frame: {e}"))?;
    let body = crate::canonical::to_vec(&f)?;
    let seq_s = seq.to_string();
    let g = key.sign_domain(dir.domain(), &[session.as_bytes(), seq_s.as_bytes(), &body])?;
    let text = serde_json::to_string(&Envelope { s: seq, f, g }).map_err(|e| format!("encode envelope: {e}"))?;
    if text.len() > MAX_FRAME_BYTES {
        return Err(format!("frame {} is {} bytes (cap {MAX_FRAME_BYTES})", frame.kind(), text.len()));
    }
    Ok(text)
}

/// Why a received envelope was rejected. Every variant closes the socket
/// with `protocol_error`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    TooLarge,
    Malformed(String),
    BadSignature,
    /// Not exactly `expected_seq`.
    Sequence { expected: u64, got: u64 },
    WrongDirection(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::TooLarge => write!(f, "frame over {MAX_FRAME_BYTES} bytes"),
            OpenError::Malformed(e) => write!(f, "malformed frame: {e}"),
            OpenError::BadSignature => write!(f, "bad frame signature"),
            OpenError::Sequence { expected, got } => write!(f, "frame sequence {got}, expected {expected}"),
            OpenError::WrongDirection(k) => write!(f, "frame {k} can't travel this way"),
        }
    }
}

/// Open an envelope from the peer whose SPKI is `peer_spki_der`. The
/// caller keeps `expected_seq` (starts at 0, +1 per accepted frame).
pub fn open(
    peer_spki_der: &[u8],
    dir: Direction,
    session: &str,
    expected_seq: u64,
    text: &str,
) -> Result<Frame, OpenError> {
    if text.len() > MAX_FRAME_BYTES {
        return Err(OpenError::TooLarge);
    }
    let env: Envelope = serde_json::from_str(text).map_err(|e| OpenError::Malformed(e.to_string()))?;
    let body = crate::canonical::to_vec(&env.f).map_err(OpenError::Malformed)?;
    let seq_s = env.s.to_string();
    if !crypto::verify_domain(peer_spki_der, dir.domain(), &[session.as_bytes(), seq_s.as_bytes(), &body], &env.g) {
        return Err(OpenError::BadSignature);
    }
    if env.s != expected_seq {
        return Err(OpenError::Sequence { expected: expected_seq, got: env.s });
    }
    let frame: Frame = serde_json::from_value(env.f).map_err(|e| OpenError::Malformed(e.to_string()))?;
    if frame.direction() != dir {
        return Err(OpenError::WrongDirection(frame.kind().to_string()));
    }
    Ok(frame)
}

// ── session frames ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Frame {
    // controller → node
    Welcome(Welcome),
    Assign(Assign),
    Transfer(Transfer),
    TransferFailed(TransferFailed),
    Cancel(Cancel),
    PolicyNarrow(PolicyNarrow),
    Revoked(Revoked),
    // node → controller
    Offer(Offer),
    JobsSnapshot(JobsSnapshot),
    Need(Need),
    State(StateUpdate),
    Log(LogChunk),
    Receipt(ReceiptFrame),
    Goodbye(Goodbye),
}

impl Frame {
    pub fn direction(&self) -> Direction {
        match self {
            Frame::Welcome(_)
            | Frame::Assign(_)
            | Frame::Transfer(_)
            | Frame::TransferFailed(_)
            | Frame::Cancel(_)
            | Frame::PolicyNarrow(_)
            | Frame::Revoked(_) => Direction::ControllerToNode,
            Frame::Offer(_)
            | Frame::JobsSnapshot(_)
            | Frame::Need(_)
            | Frame::State(_)
            | Frame::Log(_)
            | Frame::Receipt(_)
            | Frame::Goodbye(_) => Direction::NodeToController,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Frame::Welcome(_) => "welcome",
            Frame::Assign(_) => "assign",
            Frame::Transfer(_) => "transfer",
            Frame::TransferFailed(_) => "transfer_failed",
            Frame::Cancel(_) => "cancel",
            Frame::PolicyNarrow(_) => "policy_narrow",
            Frame::Revoked(_) => "revoked",
            Frame::Offer(_) => "offer",
            Frame::JobsSnapshot(_) => "jobs_snapshot",
            Frame::Need(_) => "need",
            Frame::State(_) => "state",
            Frame::Log(_) => "log",
            Frame::Receipt(_) => "receipt",
            Frame::Goodbye(_) => "goodbye",
        }
    }
}

/// First signed frame from the controller after a good handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    pub node_id: String,
    pub name: String,
    /// The controller's own label (e.g. `rosson.k2.dev`).
    pub controller: String,
    /// Jobs the controller still tracks on this node and the last log
    /// sequence it stored for each: the node replays logs after it and
    /// sends the current state (§11.5).
    pub resume: Vec<ResumeHint>,
    /// Extra routes the node may try (LAN door, tailnet), learned in-session.
    pub routes: Vec<String>,
    /// Controller-side narrowing, if any.
    pub narrow: Option<NodeLimits>,
    pub server_time: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeHint {
    pub job_id: String,
    pub generation: u32,
    pub since_seq: u64,
}

/// The attempt fence (FICC). A node never runs a stale generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fence {
    pub job_id: String,
    pub attempt: u32,
    pub generation: u32,
    /// `sha256:<hex>` of the canonical [`Plan`].
    pub plan_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assign {
    pub fence: Fence,
    pub plan: Plan,
}

impl Assign {
    /// True when `fence.plan_digest` is the digest of `plan` and the ids agree.
    pub fn consistent(&self) -> bool {
        self.fence.job_id == self.plan.job_id
            && self.plan.digest().is_ok_and(|d| crypto::ct_eq(d.as_bytes(), self.fence.plan_digest.as_bytes()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub job_id: String,
    pub node_id: String,
    pub workspace_id: String,
    /// For display and the smoke lock only.
    pub workspace_label: String,
    pub requested_by: String,
    pub argv: Vec<String>,
    /// Agent-supplied `--env` pairs (checked with `env::check_env_pairs`).
    pub env: BTreeMap<String, String>,
    /// Working directory relative to the source root (or the job dir when
    /// there's no source). Never absolute, never `..`.
    pub cwd: Option<String>,
    pub limits: JobLimits,
    pub src: Option<Src>,
    pub exclusive: bool,
    pub created_at: i64,
}

impl Plan {
    /// `sha256:<hex>` of the canonical JSON.
    pub fn digest(&self) -> Result<String, String> {
        Ok(format!("sha256:{}", crypto::sha256_hex(&crate::canonical::to_vec(self)?)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobLimits {
    pub max_secs: u64,
    /// CPU in thousandths of a core (`--cpus 4` → 4000).
    pub cpu_millis: Option<u64>,
    pub mem_bytes: Option<u64>,
    pub disk_bytes: u64,
    pub log_cap_bytes: u64,
}

/// Where the code comes from (§11.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Src {
    /// Hash of the controller workspace id + remote URL; names the mirror
    /// and the warm slots on the node.
    pub project_key: String,
    /// A remote the node may fetch history from itself. `None` = bundles only.
    pub remote_url: Option<String>,
    /// Full 40-hex commit.
    pub commit: String,
    /// The dirty patch (+ untracked tar) if `--dirty`.
    pub dirty: Option<Blob>,
    /// Warm slots the node keeps for this project (default 1 in dogfood A).
    pub slots: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blob {
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferKind {
    /// `git bundle` of `have..commit`.
    Bundle,
    /// The `--dirty` blob (patch + untracked tar; see k2-node `sync`).
    Dirty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Need {
    pub job_id: String,
    pub generation: u32,
    pub kind: TransferKind,
    /// Commit the node wants (bundle).
    pub want: String,
    /// Tips the node's mirror already has (bundle prerequisites).
    pub have: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfer {
    pub job_id: String,
    pub generation: u32,
    pub kind: TransferKind,
    /// 0-based chunk index.
    pub seq: u32,
    /// Base64 of at most [`TRANSFER_CHUNK`] raw bytes.
    pub data: String,
    pub last: bool,
    pub total_bytes: u64,
    /// SHA-256 hex of the whole blob.
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferFailed {
    pub job_id: String,
    pub generation: u32,
    pub kind: TransferKind,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cancel {
    pub job_id: String,
    pub generation: u32,
    /// `cancelled`, `timeout`, `node_removed`, …
    pub reason: String,
}

/// Controller narrowing of node caps. It can only tighten (§8.5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeLimits {
    pub cpu_millis: Option<u64>,
    pub mem_bytes: Option<u64>,
    pub max_parallel: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyNarrow {
    pub limits: NodeLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revoked {
    pub reason: String,
}

/// Node-side control state, set only by the machine's owner (§10.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Control {
    #[default]
    Active,
    Paused,
    Draining,
    Stopped,
}

impl Control {
    pub fn as_str(self) -> &'static str {
        match self {
            Control::Active => "active",
            Control::Paused => "paused",
            Control::Draining => "draining",
            Control::Stopped => "stopped",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Control::Active),
            "paused" => Some(Control::Paused),
            "draining" => Some(Control::Draining),
            "stopped" => Some(Control::Stopped),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Availability {
    pub ok: bool,
    /// Refusal codes (`outside_window`, `on_battery`, `owner_active`).
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caps {
    pub cpu_millis: u64,
    pub mem_bytes: u64,
    pub disk_budget_bytes: u64,
    /// `false` when the node can't enforce caps (no cgroup delegation,
    /// macOS memory): the offer says `soft` (CN24).
    pub hard: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Free {
    pub cpu_millis: u64,
    pub mem_bytes: u64,
    pub disk_free_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotInfo {
    pub project_key: String,
    pub slot: u32,
    pub busy: bool,
    pub last_sha: Option<String>,
}

/// What the node can take right now (§10.1). An observation, not a
/// reservation: the node rechecks at `assign` and may refuse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub revision: u64,
    pub control: Control,
    /// Who set `control` and when (unix seconds), from the node's control file.
    pub control_by: Option<String>,
    pub control_at: Option<i64>,
    pub availability: Availability,
    pub caps: Caps,
    pub free: Free,
    pub slots: Vec<SlotInfo>,
    pub running: u32,
    pub parallel_max: u32,
    /// The node-wide refusal right now (`foreign_lock`, `disk_low`, …), if any.
    pub refusal: Option<Refusal>,
    /// `os`, `arch`, `vm`, `name`, plus owner labels.
    pub labels: BTreeMap<String, String>,
    /// Tool versions (`rustc`, `cargo`, `git`, `bun`, `xcode`, `macos`);
    /// a missing tool is absent.
    pub tools: BTreeMap<String, String>,
    pub protocol: u32,
    pub boot_id: String,
    pub ledger_id: String,
    pub node_version: String,
}

/// Job states shared by the controller registry and the node ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Assigned,
    Preparing,
    Running,
    Finishing,
    /// The command ran and exited (any exit code).
    Done,
    /// Infrastructure failure (sync, prepare, disk, spawn).
    Failed,
    Cancelled,
    Timeout,
    /// Node reboot or pause-now.
    Interrupted,
    /// The controller lost contact and the node had no record.
    Unknown,
}

impl JobState {
    pub fn as_str(self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Assigned => "assigned",
            JobState::Preparing => "preparing",
            JobState::Running => "running",
            JobState::Finishing => "finishing",
            JobState::Done => "done",
            JobState::Failed => "failed",
            JobState::Cancelled => "cancelled",
            JobState::Timeout => "timeout",
            JobState::Interrupted => "interrupted",
            JobState::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => JobState::Queued,
            "assigned" => JobState::Assigned,
            "preparing" => JobState::Preparing,
            "running" => JobState::Running,
            "finishing" => JobState::Finishing,
            "done" => JobState::Done,
            "failed" => JobState::Failed,
            "cancelled" => JobState::Cancelled,
            "timeout" => JobState::Timeout,
            "interrupted" => JobState::Interrupted,
            "unknown" => JobState::Unknown,
            _ => return None,
        })
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobState::Done
                | JobState::Failed
                | JobState::Cancelled
                | JobState::Timeout
                | JobState::Interrupted
                | JobState::Unknown
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

/// What actually ran (recorded per job, §11.1 step 6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrcRan {
    pub commit: String,
    pub tree: String,
    pub dirty_sha256: Option<String>,
    pub slot: Option<u32>,
    /// `warm` (slot's last sha shared history) or `cold`.
    pub warmth: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateUpdate {
    pub job_id: String,
    pub generation: u32,
    pub state: JobState,
    /// Stable reason code (`node_reboot`, `sync_failed`, a refusal code, …).
    pub reason: Option<String>,
    /// Short human detail (never log text).
    pub detail: Option<String>,
    pub at: i64,
    pub exit: Option<ExitInfo>,
    pub src: Option<SrcRan>,
    /// Last log sequence written for this job.
    pub log_seq: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogStream {
    Out,
    Err,
    /// Node-written lines (`[k2-node] …`), e.g. the truncation marker.
    Sys,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogChunk {
    pub job_id: String,
    pub generation: u32,
    /// 1-based, continuous per job attempt.
    pub seq: u64,
    pub stream: LogStream,
    /// Base64 of at most [`MAX_LOG_CHUNK`] raw bytes.
    pub data: String,
}

/// One node ledger row, for reconciliation after a reconnect (§11.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerJob {
    pub job_id: String,
    pub generation: u32,
    pub attempt: u32,
    pub plan_digest: String,
    pub state: JobState,
    pub reason: Option<String>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub exit: Option<ExitInfo>,
    pub log_seq: u64,
    pub boot_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobsSnapshot {
    /// Rows the node still has that aren't older than its retention.
    pub jobs: Vec<LedgerJob>,
    /// True on the last page.
    pub last: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptFrame {
    pub job_id: String,
    pub generation: u32,
    pub receipt: SignedReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goodbye {
    /// `uninstall`, `shutdown`, `controller_key_changed`, …
    pub reason: String,
}

// ── receipts (§11.6) ─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub v: u32,
    pub job_id: String,
    pub attempt: u32,
    pub generation: u32,
    pub plan_digest: String,
    pub node_fp: String,
    pub controller_fp: String,
    pub workspace_id: String,
    pub argv_sha256: String,
    pub env_names: Vec<String>,
    pub src: Option<SrcRan>,
    pub tools: BTreeMap<String, String>,
    pub boot_id: String,
    pub started_at: Option<i64>,
    pub ended_at: i64,
    pub state: JobState,
    pub exit: Option<ExitInfo>,
    pub log: ReceiptLog,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptLog {
    pub sha256: String,
    pub bytes: u64,
    pub truncated: bool,
}

/// A receipt plus the node's signature over its canonical JSON. `body`
/// is kept as received so verification never re-serializes a struct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedReceipt {
    pub body: String,
    pub sig: String,
}

impl SignedReceipt {
    pub fn sign(key: &SigningKey, r: &Receipt) -> Result<Self, String> {
        let body = crate::canonical::to_string(r)?;
        let sig = key.sign_domain(DOMAIN_RECEIPT, &[body.as_bytes()])?;
        Ok(Self { body, sig })
    }

    /// The receipt, if the signature verifies against `node_spki_der`.
    pub fn verify(&self, node_spki_der: &[u8]) -> Option<Receipt> {
        if !crypto::verify_domain(node_spki_der, DOMAIN_RECEIPT, &[self.body.as_bytes()], &self.sig) {
            return None;
        }
        serde_json::from_str(&self.body).ok()
    }
}

/// `sha256` hex of the argv as canonical JSON (what receipts carry).
pub fn argv_sha256(argv: &[String]) -> String {
    crypto::sha256_hex(&crate::canonical::to_vec(&argv).unwrap_or_default())
}

// ── refusals ─────────────────────────────────────────────────────────

/// A stable refusal code plus a short sentence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    pub code: String,
    pub message: String,
}

impl Refusal {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_string(), message: message.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> Plan {
        Plan {
            job_id: "j1".into(),
            node_id: "n1".into(),
            workspace_id: "w1".into(),
            workspace_label: "k2".into(),
            requested_by: "agent:k2".into(),
            argv: vec!["cargo".into(), "test".into()],
            env: BTreeMap::from([("RUST_LOG".into(), "info".into())]),
            cwd: None,
            limits: JobLimits { max_secs: 7200, cpu_millis: None, mem_bytes: None, disk_bytes: 1 << 30, log_cap_bytes: 200 << 20 },
            src: Some(Src {
                project_key: "pk".into(),
                remote_url: Some("https://github.com/x/y.git".into()),
                commit: "a".repeat(40),
                dirty: None,
                slots: 1,
            }),
            exclusive: false,
            created_at: 1_700_000_000,
        }
    }

    #[test]
    fn golden_frames_have_stable_tags() {
        let f = Frame::Cancel(Cancel { job_id: "j".into(), generation: 2, reason: "cancelled".into() });
        assert_eq!(
            crate::canonical::to_string(&f).unwrap(),
            r#"{"generation":2,"job_id":"j","reason":"cancelled","t":"cancel"}"#
        );
        let h = HandshakeFrame::Refused(Refused { code: "node_upgrade_required".into(), message: "m".into() });
        assert_eq!(
            crate::canonical::to_string(&h).unwrap(),
            r#"{"code":"node_upgrade_required","message":"m","t":"refused"}"#
        );
        let s = Frame::State(StateUpdate {
            job_id: "j".into(),
            generation: 1,
            state: JobState::Interrupted,
            reason: Some("node_reboot".into()),
            detail: None,
            at: 5,
            exit: None,
            src: None,
            log_seq: 9,
        });
        assert!(crate::canonical::to_string(&s).unwrap().contains(r#""state":"interrupted""#));
    }

    #[test]
    fn unknown_frame_type_is_malformed() {
        let k = SigningKey::generate().unwrap();
        // Hand-sign an unknown tag so only the type check can fail.
        let f = serde_json::json!({"t": "launch_missiles"});
        let body = crate::canonical::to_vec(&f).unwrap();
        let g = k.sign_domain(DOMAIN_FRAME_C2N, &[b"sess", b"0", &body]).unwrap();
        let text = serde_json::to_string(&Envelope { s: 0, f, g }).unwrap();
        assert!(matches!(
            open(k.spki_der(), Direction::ControllerToNode, "sess", 0, &text),
            Err(OpenError::Malformed(_))
        ));
    }

    #[test]
    fn seal_open_round_trip_and_every_tamper_fails() {
        let k = SigningKey::generate().unwrap();
        let other = SigningKey::generate().unwrap();
        let f = Frame::Assign(Assign { fence: Fence { job_id: "j1".into(), attempt: 1, generation: 1, plan_digest: plan().digest().unwrap() }, plan: plan() });
        let text = seal(&k, Direction::ControllerToNode, "sess", 3, &f).unwrap();
        assert_eq!(open(k.spki_der(), Direction::ControllerToNode, "sess", 3, &text).unwrap(), f);
        // Wrong key, session, seq, direction.
        assert_eq!(open(other.spki_der(), Direction::ControllerToNode, "sess", 3, &text), Err(OpenError::BadSignature));
        assert_eq!(open(k.spki_der(), Direction::ControllerToNode, "other", 3, &text), Err(OpenError::BadSignature));
        assert_eq!(
            open(k.spki_der(), Direction::ControllerToNode, "sess", 4, &text),
            Err(OpenError::Sequence { expected: 4, got: 3 })
        );
        assert_eq!(open(k.spki_der(), Direction::NodeToController, "sess", 3, &text), Err(OpenError::BadSignature));
        // A changed argv.
        let tampered = text.replace("\"test\"", "\"tset\"");
        assert_eq!(open(k.spki_der(), Direction::ControllerToNode, "sess", 3, &tampered), Err(OpenError::BadSignature));
        // A replayed frame with a forged seq.
        let mut env: Envelope = serde_json::from_str(&text).unwrap();
        env.s = 4;
        let replay = serde_json::to_string(&env).unwrap();
        assert_eq!(open(k.spki_der(), Direction::ControllerToNode, "sess", 4, &replay), Err(OpenError::BadSignature));
    }

    #[test]
    fn seal_refuses_wrong_direction_and_oversize() {
        let k = SigningKey::generate().unwrap();
        let goodbye = Frame::Goodbye(Goodbye { reason: "x".into() });
        assert!(seal(&k, Direction::ControllerToNode, "s", 0, &goodbye).is_err());
        let big = Frame::Log(LogChunk {
            job_id: "j".into(),
            generation: 1,
            seq: 1,
            stream: LogStream::Out,
            data: "A".repeat(MAX_FRAME_BYTES + 1),
        });
        assert!(seal(&k, Direction::NodeToController, "s", 0, &big).is_err());
        assert_eq!(
            open(k.spki_der(), Direction::NodeToController, "s", 0, &"x".repeat(MAX_FRAME_BYTES + 1)),
            Err(OpenError::TooLarge)
        );
    }

    #[test]
    fn plan_digest_binds_every_field_and_assign_checks_it() {
        let p = plan();
        let d = p.digest().unwrap();
        assert!(d.starts_with("sha256:"));
        let mut q = p.clone();
        q.limits.max_secs += 1;
        assert_ne!(q.digest().unwrap(), d);
        let a = Assign { fence: Fence { job_id: "j1".into(), attempt: 1, generation: 1, plan_digest: d.clone() }, plan: p.clone() };
        assert!(a.consistent());
        let bad = Assign { fence: a.fence.clone(), plan: q };
        assert!(!bad.consistent());
        let wrong_id = Assign { fence: Fence { job_id: "j2".into(), ..a.fence.clone() }, plan: p };
        assert!(!wrong_id.consistent());
    }

    #[test]
    fn receipts_sign_verify_and_tamper_fails() {
        let k = SigningKey::generate().unwrap();
        let r = Receipt {
            v: 1,
            job_id: "j".into(),
            attempt: 1,
            generation: 1,
            plan_digest: "sha256:x".into(),
            node_fp: k.fingerprint(),
            controller_fp: "c".into(),
            workspace_id: "w".into(),
            argv_sha256: argv_sha256(&["cargo".into()]),
            env_names: vec!["CI".into()],
            src: None,
            tools: BTreeMap::new(),
            boot_id: "b".into(),
            started_at: Some(1),
            ended_at: 2,
            state: JobState::Done,
            exit: Some(ExitInfo { code: Some(1), signal: None }),
            log: ReceiptLog { sha256: "s".into(), bytes: 3, truncated: false },
        };
        let s = SignedReceipt::sign(&k, &r).unwrap();
        assert_eq!(s.verify(k.spki_der()).unwrap(), r);
        let tampered = SignedReceipt { body: s.body.replace("\"code\":1", "\"code\":0"), sig: s.sig.clone() };
        assert!(tampered.verify(k.spki_der()).is_none());
        assert!(s.verify(SigningKey::generate().unwrap().spki_der()).is_none());
    }

    #[test]
    fn session_id_depends_on_both_nonces() {
        assert_ne!(session_id("a", "b"), session_id("b", "a"));
        assert_eq!(session_id("a", "b").len(), 64);
    }

    #[test]
    fn job_state_round_trips() {
        for s in [
            JobState::Queued, JobState::Assigned, JobState::Preparing, JobState::Running, JobState::Finishing,
            JobState::Done, JobState::Failed, JobState::Cancelled, JobState::Timeout, JobState::Interrupted, JobState::Unknown,
        ] {
            assert_eq!(JobState::parse(s.as_str()), Some(s));
        }
        assert!(JobState::Done.is_terminal());
        assert!(!JobState::Running.is_terminal());
    }
}
