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
///
/// History: 1 = jobs (0.45.3); 2 = node admin (`admin`, `admin_result`,
/// `update_offer`/`update_chunk`/`update_state`, `admin_keys`, `health`;
/// prd-k2-node-admin-v1 §7.1). Additive: a protocol-1 node keeps running
/// jobs, and the controller sends admin frames only to a node whose offer
/// lists the `admin` feature. 3 = node sharing (0.46.3,
/// prd-compute-shared-queue-v1 §10; [`crate::share`]): `Offer.share`,
/// `StateUpdate.queue`, `Plan.class`…, `queue_get`/`queue_view`, the
/// `withdraw`/`if_queued` cancel reasons. Additive: a node sends none of
/// them to a server that said protocol < [`PROTOCOL_SHARED`] in
/// `Challenge.protocol`, so its frames stay 0.46.2's apart from the
/// version fields (V66). 4 = compute secret-env (0.46.4,
/// prd-compute-secret-env-v1 §6; [`crate::secret`]): `Plan.secret_env`,
/// `Receipt.secret_env`, `secret_need` / `secret_env` / `secret_refused`.
/// Additive: a node sends no secret frame to a server below 4, and a server
/// sends a secret plan only to a node whose offer lists `secret-env`, so a
/// protocol-3 peer never sees one. 5 = job tiers (0.46.5,
/// prd-compute-job-tiers-v1; [`crate::tiers`]): `Plan.tier`,
/// `ShareView.priority`, `QueueEntry.tier`, the `priority` key in
/// `servers.limits`. Additive: a node offers `tiers` (and sends the share
/// and queue fields) only to a server that said protocol ≥ 5, and a server
/// sends `Plan.tier` and the key only to a node offering `tiers`; it always
/// sets `Plan.class` from the tier too, so an older node orders by class.
/// 6 = job caches (0.46.6; [`crate::cache`]): `Plan.cache`, `cache_get` /
/// `cache_view`. Additive: a node offers `job-cache` only to a server that
/// said protocol ≥ 6, and a server sends `Plan.cache` and `cache_get` only
/// to a node offering it (an older node would refuse the digest, or drop
/// the session on a frame it can't parse), so a job with `--cache` is never
/// placed on an older node. Also 6 (same release): `Plan.src_path` and
/// `Plan.with`, sibling repos laid out side by side (`--with`), to nodes
/// offering [`FEATURE_MULTI_REPO`]. Also 6: job tiers v2 (`tiers2`,
/// prd-compute-job-tiers-v2): `Tier::Batch`, `Plan.submitter`,
/// `Plan.submitter_max_running`; a node offers `tiers2` only to a server
/// that said protocol ≥ 6 and never shows an older one a Batch tier, and a
/// server writes them only to a node offering it. Also 6 (same release,
/// compute easy mode, prd-compute-easy-mode-0467 §3.2): no new frame and
/// no plan field; a node does the easy-mode job work (CPU hints and a port
/// range in the job's env, the CPU-aware stall rule, `Outputs` on every
/// end with `tools_used` / `resources` / `host`, `Receipt.tools_used`, the
/// new `[k2-node]` log lines) and offers its features (`tools-used`,
/// `job-resources`, `stall-cpu`, `rust-components`, `toolchain-budget`,
/// `cpu-hints`, `job-ports`) only for a server that said protocol ≥ 6
/// ([`PROTOCOL_EASY`]). An older server sees its own release's frames.
///
/// 7 = self-serve environments (prd-compute-self-serve-environments-v1):
/// host facts (`Offer.host`, the `distro`/`distro_version`/`pkg`/`libc`/
/// `gpu` labels, feature `host-facts`), the package status (`Offer.pkg`,
/// `pkg-status`), distro-neutral names and profiles (`Plan.packages`,
/// `pkg-map`), package requests relayed to the node's owner
/// (`pkg_request`, `pkg_request_fwd`, `pkg_request_decide`,
/// `pkg_request_done`, `pkg-requests`) and complete Rust toolchains
/// (`rust-default-components`). Additive: a node sends the new offer
/// fields and features only to a server that said protocol ≥ 7
/// ([`crate::host::PROTOCOL_HOST`]), and a server sends the new plan field
/// and frames only to a node offering the feature. An older peer sees
/// exactly its own release's frames.
///
/// 8 = compute git access (0.46.9, prd-compute-git-access-v1; GH#78;
/// [`crate::gitcred`]): `Plan.git_access`, `git_cred_need` / `git_cred` /
/// `git_cred_refused`, the `git-access` feature. Additive: a node offers
/// `git-access` and sends `git_cred_need` only to a server that said
/// protocol ≥ [`PROTOCOL_GIT`], and a server writes `Plan.git_access` only
/// for a node offering the feature.
///
/// 9 = CI on compute for clients (prd-ci-on-compute-for-clients-v1;
/// [`crate::ci`], `docs/ci-formats-v1.md`): node self-statements, the
/// owner-signed node roster, receipt v2 (`Plan.receipt_v`, `Receipt.run`),
/// lanes, per-job uids. Defined as [`PROTOCOL_CI`] (one bump for every node
/// part, vs-live B1). Additive: a node offers `node-statement` and
/// `receipt-v2` (and later the other [`crate::ci::FEATURES`]) and sends the
/// `node_statement` frame and `ReceiptFrame.statement` only to a server
/// that said protocol ≥ 9; a server writes `Plan.receipt_v` only for a node
/// offering `receipt-v2`. A protocol-8 peer sees exactly 0.46.9's frames.
pub const PROTOCOL: u32 = 9;

/// The first protocol with the CI formats ([`crate::ci`]). Additive like
/// every bump before it: a node offers a [`crate::ci::FEATURES`] entry only
/// to a server that said protocol ≥ 9, and a server writes `Plan.receipt_v`
/// (or any other CI field or frame) only to a node offering the feature.
pub const PROTOCOL_CI: u32 = 9;

/// The first server protocol that gets the `git-access` feature and frames.
pub const PROTOCOL_GIT: u32 = 8;

/// The first server protocol that gets the easy-mode job work and features.
pub const PROTOCOL_EASY: u32 = 6;

/// The first server protocol that gets the `job-cache` feature.
pub const PROTOCOL_CACHE: u32 = 6;

/// The first server protocol that gets the `tiers2` feature (Batch, the
/// submitter fields, alternating lines and submitter turns).
pub const PROTOCOL_TIERS2: u32 = 6;

/// The first server protocol that gets the tier fields and the `tiers`
/// feature.
pub const PROTOCOL_TIERS: u32 = 5;

/// The first server protocol that may get secret-env plans and frames.
pub const PROTOCOL_SECRETS: u32 = 4;

/// The first server protocol that gets sharing fields, features and frames.
pub const PROTOCOL_SHARED: u32 = 3;

/// The oldest node protocol a server places jobs on (`offer_refusal`
/// answers `upgrade_required` below it). Not [`PROTOCOL`]: a 0.46.3 server
/// keeps using 0.46.2 (protocol 2) nodes (prd-compute-shared-queue-v1
/// §10.3).
pub const JOBS_MIN_PROTOCOL: u32 = 2;

/// Oldest node protocol a controller still attaches (jobs only).
pub const MIN_PROTOCOL: u32 = 1;

/// Largest JSON text message on either socket.
pub const MAX_FRAME_BYTES: usize = 1 << 20;
/// Largest raw log payload per `log` frame.
pub const MAX_LOG_CHUNK: usize = 64 * 1024;
/// Largest raw payload per `transfer` frame (base64 grows it by a third).
pub const TRANSFER_CHUNK: usize = 256 * 1024;
/// `artifact-flow` (GH#80): the most bytes of one artifact a node has out
/// past the controller's last `artifact_ack` (8 chunks). Over a slow relay
/// the rest waits on the node, not in the pipe ahead of every other frame.
pub const ARTIFACT_WINDOW: u64 = 8 * TRANSFER_CHUNK as u64;
/// `artifact-flow`: a node stops sending one artifact after this long
/// with no `artifact_ack` (the controller gave up or the pipe is stuck).
pub const ARTIFACT_ACK_STALL_SECS: u64 = 120;
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
    /// Protocol 9 (C1, vs-live V3): a re-keyed node proves its new key to a
    /// server that still pins the old one. The old key's signed
    /// [`crate::statement::RotationStatement`] and the new key's statement
    /// continuing it. Sent only when every active server on the node offers
    /// `node-rekey` (older servers would refuse the new key as
    /// `unknown_node`); absent otherwise, so a hello is byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<KeyRotation>,
}

/// A re-keyed node's proof for a server that pins the old key (C1, V3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyRotation {
    /// [`crate::statement::RotationStatement`], signed by the **old** key.
    pub rotation: crate::ci::Signed,
    /// The new key's [`crate::statement::NodeStatement`] (`prev_fp` = the
    /// old key, `key_epoch` = the rotation's `new_epoch`).
    pub statement: crate::ci::Signed,
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
    /// One-click path (§7.2): an enroll ticket from the control plane
    /// instead of a code. `code_binding` is then
    /// [`crate::ticket::Ticket::binding`]. Absent on the code path, so
    /// code-path frames are byte-identical to protocol 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
    /// Protocol 3: when this enrol adds a second server to a shared node,
    /// the owner server's label (`rosson.k2.dev`), so the new server shows
    /// "shared by …". Absent otherwise; an old server ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_by: Option<String>,
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
    /// Protocol 2: a signed admin command ([`crate::admin`]).
    Admin(AdminFrame),
    /// Protocol 2: a signed `k2-node` build offered for self-update.
    UpdateOffer(UpdateOffer),
    UpdateChunk(UpdateChunk),
    /// Protocol 2: approval key pin/rotation ([`crate::admin::AdminKeys`]).
    AdminKeys(crate::admin::AdminKeys),
    /// Ask for one kept artifact (only to nodes offering `artifacts`).
    ArtifactGet(ArtifactGet),
    /// Protocol 3: the node's queue as this server may see it (only to
    /// nodes offering `queue`; older parsers reject unknown variants).
    QueueGet(crate::share::QueueGet),
    /// Protocol 6: list (or clear) this server's job caches on the node
    /// (only to nodes offering `job-cache`).
    CacheGet(crate::cache::CacheGet),
    /// Protocol 4: a job's secret env, sealed to the node's one-time key
    /// (only in answer to `secret_need`).
    SecretEnv(SecretEnv),
    SecretRefused(SecretRefused),
    /// Protocol 7: ask the node's owner for system packages (only to nodes
    /// offering `pkg-requests`).
    PkgRequest(crate::pkgpolicy::PkgRequest),
    /// Protocol 7, the owner server only: deny a forwarded request.
    PkgRequestDecide(crate::pkgpolicy::PkgRequestDecide),
    /// Protocol 8: a git token for one repo, sealed to the node's one-time
    /// key (only in answer to `git_cred_need`).
    GitCred(GitCred),
    GitCredRefused(GitCredRefused),
    /// Protocol 9 (C3b, V27): the controller fetched every artifact of a
    /// secret job's attempt and checked its hash; the node deletes that
    /// attempt's kept artifacts now (only to nodes offering
    /// [`crate::ci::FEATURE_ARTIFACT_DELETE`]; older parsers reject unknown
    /// variants).
    ArtifactDelete(ArtifactDelete),
    /// GH#80 (`artifact-flow`): the controller has written an artifact up
    /// to this byte; the node may send up to `window` past it (only to
    /// nodes offering [`crate::results::FEATURE_ARTIFACT_FLOW`]; older
    /// parsers reject unknown variants).
    ArtifactAck(ArtifactAck),
    /// GH#80 (`artifact-flow`): stop sending this pull.
    ArtifactCancel(ArtifactCancel),
    // node → controller
    Offer(Offer),
    JobsSnapshot(JobsSnapshot),
    Need(Need),
    State(StateUpdate),
    Log(LogChunk),
    Receipt(ReceiptFrame),
    Goodbye(Goodbye),
    /// Protocol 2.
    AdminResult(crate::admin::AdminResult),
    UpdateState(UpdateState),
    Health(crate::health::Health),
    /// One piece of an artifact asked for with `artifact_get`.
    ArtifactChunk(ArtifactChunk),
    ArtifactFailed(ArtifactFailed),
    /// Protocol 3: the answer to `queue_get`.
    QueueView(crate::share::QueueView),
    /// Protocol 6: the answer to `cache_get`.
    CacheView(crate::cache::CacheView),
    /// Protocol 4: right before spawn, a job's secret env, for one fresh
    /// public key (only to a server that said protocol ≥ 4).
    SecretNeed(SecretNeed),
    /// Protocol 7: a package request for the owner server to decide.
    PkgRequestFwd(crate::pkgpolicy::PkgRequestFwd),
    /// Protocol 7: a request's outcome, to the server that asked.
    PkgRequestDone(crate::pkgpolicy::PkgRequestDone),
    /// Protocol 8: a git token for one granted repo of a job (only to a
    /// server that said protocol ≥ 8).
    GitCredNeed(GitCredNeed),
    /// Protocol 9: the node's signed self-statement
    /// ([`crate::statement::NodeStatement`], `docs/ci-formats-v1.md` §2),
    /// sent when a session starts and whenever it changes. Only to a server
    /// that said protocol ≥ 9 (older parsers reject unknown variants).
    NodeStatement(crate::ci::Signed),
    /// Protocol 9 (C1, V11): the owner's signed node roster and the
    /// receiving server's **own** seat statements, relayed by the node
    /// (feature `node-roster`). Only to a server that said protocol ≥ 9.
    NodeRoster(crate::lanes::NodeRosterDoc),
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
            | Frame::Revoked(_)
            | Frame::Admin(_)
            | Frame::UpdateOffer(_)
            | Frame::UpdateChunk(_)
            | Frame::AdminKeys(_)
            | Frame::ArtifactGet(_)
            | Frame::QueueGet(_)
            | Frame::CacheGet(_)
            | Frame::SecretEnv(_)
            | Frame::SecretRefused(_)
            | Frame::PkgRequest(_)
            | Frame::PkgRequestDecide(_)
            | Frame::GitCred(_)
            | Frame::GitCredRefused(_)
            | Frame::ArtifactDelete(_)
            | Frame::ArtifactAck(_)
            | Frame::ArtifactCancel(_) => Direction::ControllerToNode,
            Frame::Offer(_)
            | Frame::JobsSnapshot(_)
            | Frame::Need(_)
            | Frame::State(_)
            | Frame::Log(_)
            | Frame::Receipt(_)
            | Frame::Goodbye(_)
            | Frame::AdminResult(_)
            | Frame::UpdateState(_)
            | Frame::Health(_)
            | Frame::ArtifactChunk(_)
            | Frame::ArtifactFailed(_)
            | Frame::QueueView(_)
            | Frame::CacheView(_)
            | Frame::SecretNeed(_)
            | Frame::PkgRequestFwd(_)
            | Frame::PkgRequestDone(_)
            | Frame::GitCredNeed(_)
            | Frame::NodeStatement(_)
            | Frame::NodeRoster(_) => Direction::NodeToController,
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
            Frame::Admin(_) => "admin",
            Frame::UpdateOffer(_) => "update_offer",
            Frame::UpdateChunk(_) => "update_chunk",
            Frame::AdminKeys(_) => "admin_keys",
            Frame::AdminResult(_) => "admin_result",
            Frame::UpdateState(_) => "update_state",
            Frame::Health(_) => "health",
            Frame::ArtifactGet(_) => "artifact_get",
            Frame::ArtifactChunk(_) => "artifact_chunk",
            Frame::ArtifactFailed(_) => "artifact_failed",
            Frame::QueueGet(_) => "queue_get",
            Frame::QueueView(_) => "queue_view",
            Frame::CacheGet(_) => "cache_get",
            Frame::CacheView(_) => "cache_view",
            Frame::SecretNeed(_) => "secret_need",
            Frame::SecretEnv(_) => "secret_env",
            Frame::SecretRefused(_) => "secret_refused",
            Frame::PkgRequest(_) => "pkg_request",
            Frame::PkgRequestDecide(_) => "pkg_request_decide",
            Frame::PkgRequestFwd(_) => "pkg_request_fwd",
            Frame::PkgRequestDone(_) => "pkg_request_done",
            Frame::GitCredNeed(_) => "git_cred_need",
            Frame::GitCred(_) => "git_cred",
            Frame::GitCredRefused(_) => "git_cred_refused",
            Frame::NodeStatement(_) => "node_statement",
            Frame::NodeRoster(_) => "node_roster",
            Frame::ArtifactDelete(_) => "artifact_delete",
            Frame::ArtifactAck(_) => "artifact_ack",
            Frame::ArtifactCancel(_) => "artifact_cancel",
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

/// A job id the node may use as a folder name (`jobs/<id>-g<gen>`, the
/// journal, artifacts): 1–64 of A-Z a-z 0-9 _ - (security review A-disk
/// 2: cleanup, the disk guard and the janitor run on those folders with
/// the node's rights, so `/` or `..` would leave the server's slot).
/// K2 servers mint 32 hex.
pub fn valid_job_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

/// A project key the node may use as a folder name (`repos/<key>.git`,
/// `cache/<key>/`): 1–128 of A-Z a-z 0-9 _ - (security review B, F5).
/// K2 servers mint 16 hex.
pub fn valid_project_key(key: &str) -> bool {
    (1..=128).contains(&key.len()) && key.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

impl Assign {
    /// The job id is one the node may use as a folder name
    /// ([`valid_job_id`]).
    pub fn job_id_ok(&self) -> bool {
        valid_job_id(&self.fence.job_id)
    }

    /// No code, or code whose project key is a safe folder name
    /// ([`valid_project_key`]).
    pub fn project_key_ok(&self) -> bool {
        self.plan.src.as_ref().is_none_or(|s| valid_project_key(&s.project_key))
    }

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
    /// Toolchains and system packages the job needs ([`crate::needs`]
    /// tokens). Absent when empty, so older plans (and their digests)
    /// are unchanged; a node that doesn't know the field computes a
    /// different digest and refuses the assign instead of running
    /// without the tools.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    /// Artifact globs collected after the command exits
    /// ([`crate::results`]). Absent when empty, like `needs`: a node
    /// without the `artifacts` feature computes another digest and
    /// refuses, so the controller only sends globs to nodes that offer it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
    /// Stall timeout: no stdout/stderr for this many seconds ends the job
    /// as `timeout/stalled` (research G4). Absent = off, so older digests
    /// are unchanged; only sent to nodes offering [`FEATURE_STALL`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_secs: Option<u64>,
    /// Protocol 3, only to nodes offering `queue` (absent otherwise, so
    /// older digests are unchanged): the node queue's inputs (§7, §10.2).
    /// The class the server set from facts (`--wait` = interactive,
    /// `--detach` / big shard sets = batch, `--gate` where allowed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<crate::share::JobClass>,
    /// The submitting session (fairness inside a workspace).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The server's estimate of the run time (ETAs), p50 and p90.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub est_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub est_p90_secs: Option<u64>,
    /// The workspace's `max_parallel` on the server; the node honours it
    /// inside the tenant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ws_max_running: Option<u32>,
    /// Protocol 4: the env names of the vault secrets this job gets at
    /// spawn, sorted and unique (SE5). Names only: the values come sealed
    /// in `secret_env`, never in a plan. Absent when empty, so every other
    /// plan and digest is unchanged; sent only to nodes offering
    /// [`crate::secret::FEATURE_SECRET_ENV`] (an older node would compute
    /// another digest and fail the job `plan_mismatch`, SV14).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_env: Vec<String>,
    /// Protocol 5: the job's tier (prd-compute-job-tiers-v1). Absent unless
    /// the node offers [`crate::tiers::FEATURE_TIERS`], so every older plan
    /// and digest is unchanged (an older node would compute another digest
    /// and refuse). `class` is set beside it (`Tier::class`) for old
    /// peers; a plan without it counts as `Tier::from_class(class)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<crate::tiers::Tier>,
    /// Protocol 6: the job's named cache ([`crate::cache`]), mounted at
    /// `K2_JOB_CACHE` and kept between runs on this node. Absent without
    /// `--cache`, so every other plan and digest is unchanged; sent only to
    /// nodes offering [`crate::cache::FEATURE_JOB_CACHE`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<String>,
    /// Protocol 6, `--with` only: the main repo's folder in the workspace
    /// (`""` = the workspace's own repo). The job's code is laid out as in
    /// the workspace: `src/<src_path>` is the main checkout (and the job's
    /// start), `src/<w.path>` each [`Plan::with`] repo, so relative paths
    /// between them (`../k2-multiplayer-base`) work. Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src_path: Option<String>,
    /// Protocol 6: sibling repos shipped with the job, each at its own
    /// commit (no `--dirty` for them). Absent when empty; sent only to
    /// nodes offering [`FEATURE_MULTI_REPO`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub with: Vec<SrcWith>,
    /// Protocol 6 (`tiers2`): who sent it (`<workspace>/<session>[/<name>]`),
    /// the key of the submitter turns and the per-submitter cap. Absent
    /// unless the node offers [`crate::tiers::FEATURE_TIERS2`], so every
    /// older plan and digest is unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitter: Option<String>,
    /// Protocol 6 (`tiers2`): at most this many running jobs of this
    /// (workspace, submitter) on the node (the grant's
    /// `submitter_max_running`; absent = no cap). CI never counts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitter_max_running: Option<u32>,
    /// Protocol 7 (`pkg-map`): every system name the job needs, canonical
    /// (or `<manager>:<name>`), its `profile:` needs expanded and its `sys:`
    /// needs added, sorted and unique, at most
    /// [`crate::profiles::MAX_PLAN_PACKAGES`]. A profile counts as one need
    /// against the 32-need cap; its names ride here. Absent when empty, so
    /// every older plan and digest is unchanged; sent only to nodes
    /// offering [`crate::pkgmap::FEATURE_PKG_MAP`]. The node still checks
    /// every name against its own policy, deny list and dry run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<String>,
    /// Protocol 8 (`git-access`, GH#78): the repos this job may read with a
    /// short-lived token (`github.com/<owner>/<repo>`, lowercase, sorted,
    /// unique, at most [`crate::gitcred::MAX_REPOS`]): its workspace's live
    /// grants that cover this node, filled per node at assign. Names only:
    /// a token comes sealed in `git_cred`, never in a plan. Absent when
    /// empty, so every older plan and digest is unchanged; sent only to
    /// nodes offering [`crate::gitcred::FEATURE_GIT_ACCESS`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub git_access: Vec<String>,
    /// Protocol 9 (`receipt-v2`, prd-ci-on-compute-for-clients-v1 V7): the
    /// receipt version this job's receipt must have. Only ever `Some(2)`
    /// ([`RECEIPT_V2`]); absent asks for today's v1 receipt, so every older
    /// plan, digest and receipt is unchanged and the S3b signer path stays
    /// byte-identical (V4). Sent only to nodes offering
    /// [`crate::ci::FEATURE_RECEIPT_V2`] (an older node would compute
    /// another digest and refuse).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_v: Option<u32>,
    /// GPU jobs (0.46.11, `--gpu`): one GPU of the node, to this job alone.
    /// Absent unless asked, so every other plan and digest is unchanged;
    /// sent only to nodes offering [`crate::gpu::FEATURE_GPU`] (an older
    /// node would compute another digest and refuse the assign).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<crate::gpu::GpuAsk>,
}

impl Plan {
    /// The tier this plan asks for: `tier`, else from `class` (an older
    /// server's plan), else Normal.
    pub fn tier_or_class(&self) -> crate::tiers::Tier {
        self.tier.unwrap_or_else(|| crate::tiers::Tier::from_class(self.class.unwrap_or_default()))
    }

    /// The submitter-turns key: the submitter, else the session (an older
    /// server's plan), else nothing.
    pub fn turn_key(&self) -> Option<String> {
        self.submitter.clone().or_else(|| self.session_id.clone())
    }
}

/// `secret_need` (node → controller, protocol 4): the node is about to
/// spawn this attempt and asks for its secret env, sealed to `epk` (base64
/// X25519, fresh for this ask).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretNeed {
    pub job_id: String,
    pub generation: u32,
    pub attempt: u32,
    pub plan_digest: String,
    pub epk: String,
}

/// `secret_env` (controller → node): `ct` = AES-256-GCM of the payload
/// under the key both one-time halves give ([`crate::secret`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretEnv {
    pub job_id: String,
    pub generation: u32,
    pub epk: String,
    pub ct: String,
}

/// `secret_refused` (controller → node): no release; `code` is one of
/// [`crate::secret::codes`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretRefused {
    pub job_id: String,
    pub generation: u32,
    pub code: String,
    pub message: String,
}

/// `git_cred_need` (node → server, protocol 8): a git token for `repo`
/// (one of the plan's `git_access`), sealed to `epk` (base64 X25519, fresh
/// for this ask). `req` is the node's own counter: one job may wait on
/// several repos at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCredNeed {
    pub job_id: String,
    pub generation: u32,
    pub attempt: u32,
    pub plan_digest: String,
    pub req: u64,
    pub repo: String,
    pub purpose: crate::gitcred::Purpose,
    pub epk: String,
}

/// `git_cred` (server → node): `ct` = AES-256-GCM of the credential under
/// the key both one-time halves give ([`crate::gitcred`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCred {
    pub job_id: String,
    pub generation: u32,
    pub req: u64,
    pub epk: String,
    pub ct: String,
}

/// `git_cred_refused` (server → node): no token; `code` is one of
/// [`crate::gitcred::codes`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCredRefused {
    pub job_id: String,
    pub generation: u32,
    pub req: u64,
    pub code: String,
    pub message: String,
}

/// Offer feature: this node ends a job that prints nothing for
/// `Plan::stall_secs`.
pub const FEATURE_STALL: &str = "stall";

/// Offer feature (0.46.7, CE-10, D4): a stall is silent **and** CPU-idle.
/// The node ends a job as `timeout/stalled` only when it printed nothing
/// for `Plan::stall_secs` and its processes together used under 1% of one
/// CPU over that window; a silent job that burns CPU keeps running and
/// gets a `[k2-node] quiet but busy` log line every [`QUIET_BUSY_NOTE_SECS`].
/// An older node ends on silence alone. No plan field changes.
pub const FEATURE_STALL_CPU: &str = "stall-cpu";

/// How often a quiet-but-busy job gets its log line (CE-10).
pub const QUIET_BUSY_NOTE_SECS: u64 = 30 * 60;

/// Offer feature: jobs run behind the job wall (Linux: the `k2job` user in
/// a sandboxed systemd unit; macOS: a Seatbelt profile). A job can't read
/// the node's key or state, or other people's home folders (0.46.3).
pub const FEATURE_JOB_WALL: &str = "job-wall";
/// Offer feature: this node enforces `network = "none"` (no job may open a
/// network connection beyond loopback), so the owner may set it.
pub const FEATURE_NETWORK_NONE: &str = "network-none";

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

/// One sibling repo of a `--with` job (protocol 6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrcWith {
    /// Its folder in the workspace (relative, plain names: [`valid_layout_path`]).
    pub path: String,
    /// Names its mirror on the node, like [`Src::project_key`].
    pub project_key: String,
    pub remote_url: Option<String>,
    /// Full 40-hex commit.
    pub commit: String,
}

/// Offer feature (protocol 6): this node lays out `Plan.with` repos beside
/// the main one.
pub const FEATURE_MULTI_REPO: &str = "multi-repo";

/// Most `--with` repos per job.
pub const MAX_WITH: usize = 8;

/// A folder a repo is laid out at in a job: relative, 1–512 bytes, plain
/// names only (no `.`, `..`, empty parts or `.k2`), no NUL or backslash.
/// `""` is allowed only as `Plan.src_path` (the workspace's own repo).
pub fn valid_layout_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 512
        && !p.contains('\0')
        && !p.contains('\\')
        && p.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && c != ".k2")
}

/// The `--with` layout is sound: every path valid, none equal to or inside
/// another's (the main repo's `""` holds them all), the main's `src_path`
/// set. `Err` names the problem.
pub fn check_layout(src_path: &str, with: &[SrcWith]) -> Result<(), String> {
    if with.len() > MAX_WITH {
        return Err(format!("at most {MAX_WITH} --with repos"));
    }
    if !src_path.is_empty() && !valid_layout_path(src_path) {
        return Err(format!("the main repo's folder {src_path:?} isn't a plain relative path"));
    }
    let mut all: Vec<&str> = with.iter().map(|w| w.path.as_str()).collect();
    for w in with {
        if !valid_layout_path(&w.path) {
            return Err(format!("--with folder {:?} isn't a plain relative path", w.path));
        }
    }
    if !src_path.is_empty() {
        all.push(src_path);
    }
    for (i, a) in all.iter().enumerate() {
        for b in all.iter().skip(i + 1) {
            let inside = |x: &str, y: &str| x == y || x.starts_with(&format!("{y}/"));
            if inside(a, b) || inside(b, a) {
                return Err(format!("repos {a:?} and {b:?} would overlap in the job's folder"));
            }
        }
    }
    Ok(())
}

/// The second half of the `--with` layout check, run on the node once the
/// main checkout (and any dirty patch) is in place. When the main repo is
/// the layout root (`src_path` `""`), every `--with` repo lands inside its
/// checkout, and syncing one first removes what is at its path. So a path
/// the main checkout already has (`exists`) is refused: a `--with` repo
/// never replaces the main repo's files, and the receipt's tree stays the
/// tree that ran (S3b security review H1). A path the main repo doesn't
/// have (a nested repo it ignores) is still fine. `Err` names the path.
pub fn check_with_in_main_checkout(src_path: &str, with: &[SrcWith], exists: impl Fn(&str) -> bool) -> Result<(), String> {
    if !src_path.is_empty() {
        return Ok(());
    }
    for w in with {
        if exists(&w.path) {
            return Err(format!(
                "--with {:?} is inside the main checkout, which already has that path: a --with repo never replaces the main repo's files",
                w.path
            ));
        }
    }
    Ok(())
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
    /// `cancelled`, `timeout`, `node_removed`, …, or one of the "only if
    /// not started" reasons [`CANCEL_WITHDRAW`] / [`CANCEL_IF_QUEUED`].
    pub reason: String,
}

/// Protocol 3 (only to nodes offering `queue`): take a node-queued job
/// back to move it. A started job is never touched; the node answers its
/// real state, `cancelled/withdrawn` when it was still queued (V46, V57).
pub const CANCEL_WITHDRAW: &str = "withdraw";
/// Protocol 3: a cancel without `--force`. Same rule as withdraw; ends
/// `cancelled` when it was still queued (V57).
pub const CANCEL_IF_QUEUED: &str = "if_queued";

/// The node's answers (V46, V57):
/// - `withdraw` on a queued job → `cancelled` / `withdrawn`;
/// - `if_queued` on a queued job → `cancelled` / `cancelled`;
/// - either on a started job → its real state (`preparing`, `running`, …),
///   the job untouched; on an ended one → its row; unknown → `unknown` /
///   `not_on_node`.
/// The node may also end queued rows by itself: `cancelled` / `superseded`
/// (not in `Welcome.resume`, V47), `share_expired`, `share_revoked`.
///
/// Must a cancel with this reason leave a started job alone?
pub fn cancel_only_if_not_started(reason: &str) -> bool {
    reason == CANCEL_WITHDRAW || reason == CANCEL_IF_QUEUED
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
    /// GPU jobs (0.46.11): GPUs this session's server may use here. Absent
    /// = 0 (a node without GPU jobs, or an older one), so every older
    /// offer is unchanged and a new server parses an old node's (vs-live A8).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gpus: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Free {
    pub cpu_millis: u64,
    pub mem_bytes: u64,
    pub disk_free_bytes: u64,
    /// GPU jobs (0.46.11): of `caps.gpus`, how many no job holds now.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gpus: u32,
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
    /// What this node understands beyond protocol 1 (`needs`,
    /// `system-packages`). Older nodes send none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// Protocol 3: this session's share (V53). Absent for a server that
    /// said protocol < 3, and on a node nobody shares.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<crate::share::ShareView>,
    /// Protocol 7: what this node is ([`crate::host`]). Absent for an older
    /// server (the solo goldens hold it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<crate::host::HostFacts>,
    /// Protocol 7: this server's view of the node's system packages
    /// ([`crate::pkgpolicy::PkgStatus`]). Linux nodes with the package
    /// installer only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pkg: Option<crate::pkgpolicy::PkgStatus>,
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
    /// Artifacts and test results (terminal states, `artifacts` nodes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<crate::results::Outputs>,
    /// Protocol 3: position and ETA while the job waits in the node queue
    /// (state `queued`, reason `node_queued`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<crate::share::QueuePos>,
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
    /// Protocol 9, receipt v2 only: the node self-statement its
    /// `run.statement_digest` names (the one in force when the job
    /// started), so the controller can keep it beside the receipt for the
    /// evidence bundle. Absent for a v1 receipt, so a protocol-8 server
    /// sees 0.46.9's frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statement: Option<crate::ci::Signed>,
}

/// `artifact_get`: send one kept artifact of an attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactGet {
    /// The controller's id for this pull (echoed in every answer).
    pub request_id: String,
    pub job_id: String,
    pub generation: u32,
    /// An [`crate::results::ArtifactInfo::name`] the node reported.
    pub name: String,
    /// GH#80 (`artifact-flow`): start at this byte (a retry continues a
    /// partial pull). Absent: from the start, as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    /// GH#80 (`artifact-flow`): the most bytes the node may send past the
    /// last [`ArtifactAck`]. Absent: the whole file at once, as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<u64>,
}

/// `artifact_ack` (GH#80, `artifact-flow`): the controller holds the file
/// up to byte `next` (from the start of the file, not the request).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactAck {
    pub request_id: String,
    pub next: u64,
}

/// `artifact_cancel` (GH#80, `artifact-flow`): the controller stopped
/// this pull (it timed out, failed, or the session that asked is gone);
/// the node stops sending it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactCancel {
    pub request_id: String,
}

/// `artifact_delete` (protocol 9, V27): delete one attempt's kept
/// artifacts. Sent by the controller of a secret job on a node it doesn't
/// own, once it holds every artifact of that attempt, hash-checked. The
/// node deletes only that server's own copy (its tenant's artifacts dir),
/// and only for an attempt its ledger knows; anything else is ignored.
/// Kept artifacts nobody fetched still go at the node's expiry (7 days).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactDelete {
    pub job_id: String,
    pub generation: u32,
}

/// `artifact_chunk`: ≤ [`TRANSFER_CHUNK`] raw bytes of the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactChunk {
    pub request_id: String,
    pub seq: u32,
    pub data: String,
    pub last: bool,
    pub total_bytes: u64,
    /// SHA-256 hex of the whole file (as kept).
    pub sha256: String,
}

/// `artifact_failed`: the node can't send it (gone, cap, read error).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactFailed {
    pub request_id: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goodbye {
    /// `uninstall`, `shutdown`, `controller_key_changed`, …
    pub reason: String,
}

// ── node admin (protocol 2) ──────────────────────────────────────────

/// `admin`: one signed command. The node passes `cmd` to its root
/// executor untouched (or answers read verbs itself).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminFrame {
    pub cmd: crate::admin::SignedCommand,
}

/// `update_offer` (prd-k2-node-admin-v1 §8.2 step 2): one signed build
/// for this node's os/arch. `sig` is the minisign text (root-helper key,
/// trusted comment `file:k2-node-<os>-<arch> version:<v> protocol:<p>`).
/// `cmd` is a controller-signed `node.update` command whose args name
/// this exact build (`version`, `sha256`, `trigger`). Bytes follow as
/// `update_chunk` frames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateOffer {
    pub transfer_id: String,
    pub version: String,
    pub protocol: u32,
    pub os: String,
    pub arch: String,
    pub sha256: String,
    pub size: u64,
    pub sig: String,
    pub cmd: crate::admin::SignedCommand,
}

/// One piece of an offered build (≤ [`TRANSFER_CHUNK`] raw bytes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateChunk {
    pub transfer_id: String,
    pub seq: u32,
    pub data: String,
    pub last: bool,
}

/// `update_state`: staged / swapped / healthy / rolled_back / refused / failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateState {
    #[serde(default)]
    pub transfer_id: Option<String>,
    #[serde(default)]
    pub cmd_id: Option<String>,
    pub phase: crate::admin::UpdatePhase,
    /// The version this phase is about.
    pub version: String,
    /// The version running before.
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    pub at: i64,
    /// The executor's update steps (0.46.4). Empty from older nodes, and
    /// left off the wire when empty so older controllers see the same frame.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<crate::admin::UpdateStep>,
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
    /// The files kept as artifacts (§11.6). Absent when none, so older
    /// receipts are byte-identical.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<crate::results::ArtifactInfo>,
    /// Protocol 4: the env names the job got from the vault (SE29). Names
    /// only; absent when none, so other receipts are byte-identical.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_env: Vec<String>,
    /// 0.46.7 (CE-4, `tools-used`): the versions the job's own PATH
    /// resolved to ([`crate::results::Outputs::tools_used`]), signed.
    /// `tools` above stays the node-wide probe. Absent when empty, so an
    /// older node's receipts are byte-identical.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tools_used: BTreeMap<String, String>,
    /// Receipt v2 only (protocol 9, `Plan.receipt_v = 2`): how and where the
    /// job ran ([`crate::ci::RunFacts`]). Absent in a v1 receipt, so v1
    /// receipts stay byte-identical (V7). [`Receipt::check_version`] ties
    /// it to `v`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<crate::ci::RunFacts>,
}

/// Receipt v1: every receipt before protocol 9, and every receipt whose plan
/// has no `receipt_v`.
pub const RECEIPT_V1: u32 = 1;
/// Receipt v2: `run` is present. Sent only when the plan asks for it.
pub const RECEIPT_V2: u32 = 2;

impl Receipt {
    /// `v` and the v2 facts agree: v1 has no `run`, v2 has one (and its
    /// facts are well formed). Anything else is refused by name: a verifier
    /// never guesses which schema a signed body meant.
    pub fn check_version(&self) -> Result<(), String> {
        match (self.v, &self.run) {
            (RECEIPT_V1, None) => Ok(()),
            (RECEIPT_V1, Some(_)) => Err("receipt v1 carries v2 run facts".into()),
            (RECEIPT_V2, Some(run)) => run.check(),
            (RECEIPT_V2, None) => Err("receipt v2 without its run facts".into()),
            (v, _) => Err(format!("receipt version {v} is not 1 or 2")),
        }
    }

    /// Receipt v2: `st` is the node self-statement `run.statement_digest`
    /// names, signed by this receipt's own node key at the same key epoch;
    /// returned opened. Whose key that is stays the caller's pin (the
    /// receipt's own signature is checked against it).
    pub fn check_statement(&self, st: &crate::ci::Signed) -> Result<crate::statement::NodeStatement, String> {
        let run = self.run.as_ref().ok_or("a v1 receipt names no statement")?;
        if st.digest() != run.statement_digest {
            return Err("the statement is not the one the receipt names (statement_digest)".into());
        }
        let doc = crate::statement::NodeStatement::open(st)?;
        if doc.node_fp != self.node_fp {
            return Err("the statement is another node key's".into());
        }
        if doc.key_epoch != run.key_epoch {
            return Err("the statement's key epoch is not the receipt's".into());
        }
        Ok(doc)
    }
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

    #[test]
    fn job_ids_are_folder_safe() {
        let long_ok = "a".repeat(64);
        for ok in ["a", "0123456789abcdef0123456789abcdef", "job_1-x", long_ok.as_str()] {
            assert!(valid_job_id(ok), "{ok}");
        }
        let long_bad = "a".repeat(65);
        for bad in ["", "../x", "a/b", "..", ".", "a b", "a\\b", "j\u{e9}", "x\0", long_bad.as_str()] {
            assert!(!valid_job_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn project_keys_are_folder_safe() {
        assert!(valid_project_key("7bc5eeaf323643cc") && valid_project_key("pk_1-x"));
        let long_bad = "a".repeat(129);
        for bad in ["", "../x", "a/b", "..", "a b", "x\0", long_bad.as_str()] {
            assert!(!valid_project_key(bad), "{bad:?}");
        }
    }

    fn plan() -> Plan {
        Plan {
            class: None,
            session_id: None,
            est_secs: None,
            est_p90_secs: None,
            ws_max_running: None,
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
            needs: Vec::new(),
            artifacts: Vec::new(),
            stall_secs: None,
            secret_env: Vec::new(),
            tier: None,
            cache: None,
            src_path: None, submitter: None, submitter_max_running: None, packages: Vec::new(), git_access: Vec::new(),
            with: Vec::new(),
            receipt_v: None,
            gpu: None,
        }
    }

    /// GH#80: `artifact-flow` is additive. A get without `offset` and
    /// `window` is the old wire form exactly (an older node parses it and
    /// sends the whole file); an older controller's get parses here with
    /// neither; the ack and cancel go controller → node only.
    #[test]
    fn artifact_flow_is_additive_for_older_peers() {
        let old = ArtifactGet { request_id: "r".into(), job_id: "j".into(), generation: 1, name: "dist/a".into(), offset: None, window: None };
        let text = serde_json::to_string(&Frame::ArtifactGet(old.clone())).unwrap();
        assert_eq!(text, r#"{"t":"artifact_get","request_id":"r","job_id":"j","generation":1,"name":"dist/a"}"#);
        let parsed: Frame = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, Frame::ArtifactGet(old));
        let new = ArtifactGet { request_id: "r".into(), job_id: "j".into(), generation: 1, name: "dist/a".into(), offset: Some(5), window: Some(ARTIFACT_WINDOW) };
        let back: Frame = serde_json::from_str(&serde_json::to_string(&Frame::ArtifactGet(new.clone())).unwrap()).unwrap();
        assert_eq!(back, Frame::ArtifactGet(new));
        let key = SigningKey::generate().unwrap();
        for f in [Frame::ArtifactAck(ArtifactAck { request_id: "r".into(), next: 7 }), Frame::ArtifactCancel(ArtifactCancel { request_id: "r".into() })] {
            assert_eq!(f.direction(), Direction::ControllerToNode, "{}", f.kind());
            let sealed = seal(&key, Direction::ControllerToNode, "s", 0, &f).unwrap();
            assert_eq!(open(key.spki_der(), Direction::ControllerToNode, "s", 0, &sealed).unwrap(), f);
            assert!(seal(&key, Direction::NodeToController, "s", 0, &f).is_err(), "never node → controller");
        }
        assert_eq!(Frame::ArtifactAck(ArtifactAck { request_id: "r".into(), next: 0 }).kind(), "artifact_ack");
        assert_eq!(Frame::ArtifactCancel(ArtifactCancel { request_id: "r".into() }).kind(), "artifact_cancel");
    }

    /// Protocol 7 with older peers (prd-compute-self-serve-environments
    /// §6.5, §13): an empty `packages` leaves a plan's wire form (and so its
    /// digest) as before; an older node's offer (no `host`, no `pkg`)
    /// parses; the new frames carry their own tags and directions.
    #[test]
    fn protocol_seven_is_additive_for_older_peers() {
        let p = plan();
        let text = serde_json::to_string(&p).unwrap();
        assert!(!text.contains("packages"), "{text}");
        let mut q = plan();
        q.packages = vec!["libssl-dev".into()];
        assert!(serde_json::to_string(&q).unwrap().contains("\"packages\":[\"libssl-dev\"]"));
        let old_plan: Plan = serde_json::from_str(&text).unwrap();
        assert!(old_plan.packages.is_empty());
        // A 0.46.8 offer: no host, no pkg.
        let old = r#"{"revision":1,"control":"active","control_by":null,"control_at":null,"availability":{"ok":true,"reasons":[]},
            "caps":{"cpu_millis":1000,"mem_bytes":1,"disk_budget_bytes":1,"hard":true},"free":{"cpu_millis":1,"mem_bytes":1,"disk_free_bytes":1},
            "slots":[],"running":0,"parallel_max":1,"refusal":null,"labels":{"os":"linux","arch":"x86_64"},"tools":{},"protocol":6,
            "boot_id":"b","ledger_id":"l","node_version":"0.46.8","features":["needs"]}"#;
        let o: Offer = serde_json::from_str(old).unwrap();
        assert!(o.host.is_none() && o.pkg.is_none());
        let back = serde_json::to_string(&o).unwrap();
        assert!(!back.contains("\"host\"") && !back.contains("\"pkg\""), "an older server sees no new keys: {back}");
        let f = Frame::PkgRequest(crate::pkgpolicy::PkgRequest { id: "cpr_1".into(), names: vec!["libssl-dev".into()], job_id: None, why: None, asked_by: "tests".into() });
        assert_eq!((f.kind(), f.direction()), ("pkg_request", Direction::ControllerToNode));
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["t"], "pkg_request");
        let d = Frame::PkgRequestDone(crate::pkgpolicy::PkgRequestDone { server_request: "cpr_1".into(), state: "denied".into(), note: None, installed: vec![], owner: "acme.k2.dev".into() });
        assert_eq!((d.kind(), d.direction()), ("pkg_request_done", Direction::NodeToController));
        // The request frame refuses unknown keys (a node never guesses).
        assert!(serde_json::from_str::<crate::pkgpolicy::PkgRequest>(r#"{"id":"x","names":[],"asked_by":"a","shell":"x"}"#).is_err());
    }

    /// GPU jobs (0.46.11): a plan without `--gpu` has today's wire form and
    /// digest; with it, the digest changes (an older node refuses it rather
    /// than run without the GPU). An offer without GPUs has no new keys, and
    /// a new server reads an older node's offer as 0 GPUs (vs-live A8).
    #[test]
    fn plan_digest_unchanged_without_gpu() {
        let p = plan();
        let text = serde_json::to_string(&p).unwrap();
        assert!(!text.contains("gpu"), "{text}");
        let before = p.digest().unwrap();
        let mut g = plan();
        g.gpu = Some(crate::gpu::GpuAsk::one(crate::gpu::GpuVendor::Any));
        assert!(serde_json::to_string(&g).unwrap().contains(r#""gpu":{"vendor":"any","count":1}"#));
        assert_ne!(g.digest().unwrap(), before, "a GPU plan has its own digest");
        g.gpu = None;
        assert_eq!(g.digest().unwrap(), before);
        let old = r#"{"revision":1,"control":"active","control_by":null,"control_at":null,"availability":{"ok":true,"reasons":[]},
            "caps":{"cpu_millis":1000,"mem_bytes":1,"disk_budget_bytes":1,"hard":true},"free":{"cpu_millis":1,"mem_bytes":1,"disk_free_bytes":1},
            "slots":[],"running":0,"parallel_max":1,"refusal":null,"labels":{},"tools":{},"protocol":9,
            "boot_id":"b","ledger_id":"l","node_version":"0.46.10"}"#;
        let o: Offer = serde_json::from_str(old).unwrap();
        assert_eq!((o.caps.gpus, o.free.gpus), (0, 0));
        let back = serde_json::to_string(&o).unwrap();
        assert!(!back.contains("gpus"), "no GPU keys without GPUs: {back}");
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
            queue: None,
            job_id: "j".into(),
            generation: 1,
            state: JobState::Interrupted,
            reason: Some("node_reboot".into()),
            detail: None,
            at: 5,
            exit: None,
            src: None,
            log_seq: 9,
            outputs: None,
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
        let wrong_id = Assign { fence: Fence { job_id: "j2".into(), ..a.fence.clone() }, plan: p.clone() };
        assert!(!wrong_id.consistent());
        // Empty artifact globs leave the canonical plan (and every older
        // digest) unchanged; any glob changes it.
        let text = crate::canonical::to_string(&p).unwrap();
        assert!(!text.contains("artifacts") && !text.contains("stall_secs"), "{text}");
        let mut g = p.clone();
        g.artifacts = vec!["out/*.xml".into()];
        assert_ne!(g.digest().unwrap(), d);
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
            artifacts: Vec::new(),
            secret_env: Vec::new(),
            tools_used: BTreeMap::new(),
            run: None,
        };
        // Without secrets: byte-identical to a 0.46.3 receipt (SE29).
        assert!(!crate::canonical::to_string(&r).unwrap().contains("secret_env"));
        // Without tools_used: byte-identical to a 0.46.6 receipt (CE-4);
        // with it, signed like every other field.
        assert!(!crate::canonical::to_string(&r).unwrap().contains("tools_used"));
        let mut rt = r.clone();
        rt.tools_used.insert("rustc".into(), "1.99.0".into());
        let st = SignedReceipt::sign(&k, &rt).unwrap();
        assert_eq!(st.verify(k.spki_der()).unwrap().tools_used.get("rustc").map(String::as_str), Some("1.99.0"));
        let forged = SignedReceipt { body: st.body.replace("1.99.0", "1.99.1"), sig: st.sig.clone() };
        assert!(forged.verify(k.spki_der()).is_none());
        let s = SignedReceipt::sign(&k, &r).unwrap();
        assert_eq!(s.verify(k.spki_der()).unwrap(), r);
        let tampered = SignedReceipt { body: s.body.replace("\"code\":1", "\"code\":0"), sig: s.sig.clone() };
        assert!(tampered.verify(k.spki_der()).is_none());
        assert!(s.verify(SigningKey::generate().unwrap().spki_der()).is_none());
    }

    /// The release signer (scripts/k2sign/k2sign.py, prd-signing-user-v1
    /// S3b) checks receipts with openssl and its own canonical JSON. This
    /// vector was signed by openssl over the bytes the signer builds; it
    /// must verify here, and the Rust canonical form of the parsed receipt
    /// must be the very body (escapes included), or the two sides disagree.
    #[test]
    fn receipt_vector_signed_by_openssl_verifies_and_is_canonical() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/release/fixtures/receipt-vector.json");
        let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let spki = crypto::pem_decode("PUBLIC KEY", v["public_key_pem"].as_str().unwrap()).unwrap();
        assert_eq!(crypto::fingerprint_of_spki_der(&spki), v["node_fp"].as_str().unwrap());
        let s = SignedReceipt { body: v["body"].as_str().unwrap().into(), sig: v["signature"].as_str().unwrap().into() };
        let r = s.verify(&spki).expect("openssl's signature over the receipt domain bytes verifies");
        assert_eq!(crate::canonical::to_string(&r).unwrap(), s.body, "Rust's canonical form is the signed body");
        assert_eq!(r.argv_sha256, argv_sha256(&["bash".into(), "scripts/ci/build-linux-bins.sh".into(), "x86_64".into()]));
        let tampered = SignedReceipt { body: s.body.replace("\"code\":0", "\"code\":1"), sig: s.sig.clone() };
        assert!(tampered.verify(&spki).is_none());
    }

    /// Protocol 3 fields are absent when unset: a 0.46.2 plan's digest,
    /// offer, state and enrol request are unchanged (V66).
    #[test]
    fn sharing_fields_are_absent_when_unset() {
        let p = plan();
        let text = crate::canonical::to_string(&p).unwrap();
        for f in ["class", "session_id", "est_secs", "est_p90_secs", "ws_max_running"] {
            assert!(!text.contains(f), "{f} in {text}");
        }
        // This plan's digest as the v0.46.2 tag's k2-node-proto computes it.
        assert_eq!(p.digest().unwrap(), "sha256:31d079662290d8b6e10bc3595162f277743d060a50046b1be2f020e99d948e14");
        let mut q = p.clone();
        q.class = Some(crate::share::JobClass::Batch);
        assert_ne!(q.digest().unwrap(), p.digest().unwrap(), "class is bound by the digest");
        assert!(crate::canonical::to_string(&q).unwrap().contains(r#""class":"batch""#));
        let s = StateUpdate {
            queue: None,
            job_id: "j".into(),
            generation: 1,
            state: JobState::Queued,
            reason: Some("node_queued".into()),
            detail: None,
            at: 1,
            exit: None,
            src: None,
            log_seq: 0,
            outputs: None,
        };
        assert!(!crate::canonical::to_string(&s).unwrap().contains("queue\""));
        let e = EnrollRequest {
            shared_by: None,
            code_binding: "b".into(),
            node_public_key_pem: "k".into(),
            name: "mini-1".into(),
            labels: BTreeMap::new(),
            protocol: PROTOCOL,
            node_version: "0.46.3".into(),
            ticket: None,
        };
        assert!(!crate::canonical::to_string(&HandshakeFrame::EnrollRequest(e)).unwrap().contains("shared_by"));
    }

    #[test]
    fn queue_frames_travel_their_way() {
        let k = SigningKey::generate().unwrap();
        let get = Frame::QueueGet(crate::share::QueueGet { request_id: "r1".into() });
        assert_eq!((get.direction(), get.kind()), (Direction::ControllerToNode, "queue_get"));
        let t = seal(&k, Direction::ControllerToNode, "s", 0, &get).unwrap();
        assert_eq!(open(k.spki_der(), Direction::ControllerToNode, "s", 0, &t).unwrap(), get);
        let view = Frame::QueueView(crate::share::QueueView {
            request_id: "r1".into(),
            at: 5,
            entries: vec![crate::share::QueueEntry {
                pos: 1,
                mine: false,
                class: crate::share::JobClass::Gate,
                state: JobState::Running,
                cpu_millis: 8000,
                mem_bytes: 1 << 30,
                eta_secs: Some(360),
                eta_p90_secs: None,
                blocked_by: None,
                job_id: None,
                generation: None,
                workspace_id: None,
                session_id: None,
                server_label: None,
                tier: None,
            }],
            totals: vec![],
        });
        assert_eq!((view.direction(), view.kind()), (Direction::NodeToController, "queue_view"));
        assert!(seal(&k, Direction::ControllerToNode, "s", 0, &view).is_err());
        let text = crate::canonical::to_string(&view).unwrap();
        for hidden in ["job_id", "workspace_id", "session_id", "server_label", "totals"] {
            assert!(!text.contains(hidden), "a redacted entry carries no {hidden}: {text}");
        }
    }

    #[test]
    fn only_withdraw_and_if_queued_spare_a_started_job() {
        assert!(cancel_only_if_not_started(CANCEL_WITHDRAW));
        assert!(cancel_only_if_not_started(CANCEL_IF_QUEUED));
        for r in ["cancelled", "timeout", "node_removed", "superseded", ""] {
            assert!(!cancel_only_if_not_started(r), "{r}");
        }
        const { assert!(PROTOCOL >= PROTOCOL_SECRETS && PROTOCOL_SECRETS > PROTOCOL_SHARED && PROTOCOL_SHARED > 2 && MIN_PROTOCOL == 1) };
    }

    /// Protocol 4 (SE5): a plan without secrets has the 0.46.2/0.46.3
    /// digest; names change it; the secret frames travel their own way and
    /// never carry anything but names, keys and ciphertext.
    #[test]
    fn secret_env_is_absent_when_unset_and_frames_travel_their_way() {
        let p = plan();
        assert!(!crate::canonical::to_string(&p).unwrap().contains("secret_env"));
        assert_eq!(p.digest().unwrap(), "sha256:31d079662290d8b6e10bc3595162f277743d060a50046b1be2f020e99d948e14");
        let mut q = p.clone();
        q.secret_env = vec!["BUILD_TOKEN".into()];
        assert_ne!(q.digest().unwrap(), p.digest().unwrap());
        assert!(crate::canonical::to_string(&q).unwrap().contains(r#""secret_env":["BUILD_TOKEN"]"#));
        let k = SigningKey::generate().unwrap();
        let need = Frame::SecretNeed(SecretNeed { job_id: "j".into(), generation: 1, attempt: 1, plan_digest: "sha256:x".into(), epk: "e".into() });
        assert_eq!((need.direction(), need.kind()), (Direction::NodeToController, "secret_need"));
        assert!(seal(&k, Direction::ControllerToNode, "s", 0, &need).is_err());
        let t = seal(&k, Direction::NodeToController, "s", 0, &need).unwrap();
        assert_eq!(open(k.spki_der(), Direction::NodeToController, "s", 0, &t).unwrap(), need);
        for f in [
            Frame::SecretEnv(SecretEnv { job_id: "j".into(), generation: 1, epk: "e".into(), ct: "c".into() }),
            Frame::SecretRefused(SecretRefused { job_id: "j".into(), generation: 1, code: crate::secret::codes::NOT_GRANTED.into(), message: "m".into() }),
        ] {
            assert_eq!(f.direction(), Direction::ControllerToNode);
            assert!(seal(&k, Direction::NodeToController, "s", 0, &f).is_err());
        }
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

    /// Protocol 5 (prd-compute-job-tiers-v1 §3.6, T11): a plan without a
    /// tier has the 0.46.2–0.46.4 digest (an old node keeps taking it); a
    /// tier changes the digest (an old node refuses a tiered plan instead
    /// of running it wrong), and a plan without one counts by its class.
    #[test]
    fn tier_is_absent_when_unset_and_bound_by_the_digest() {
        let p = plan();
        assert!(!crate::canonical::to_string(&p).unwrap().contains("tier"));
        assert_eq!(p.digest().unwrap(), "sha256:31d079662290d8b6e10bc3595162f277743d060a50046b1be2f020e99d948e14");
        assert_eq!(p.tier_or_class(), crate::tiers::Tier::Normal);
        let mut q = p.clone();
        q.tier = Some(crate::tiers::Tier::Priority);
        assert_ne!(q.digest().unwrap(), p.digest().unwrap(), "the tier is bound by the digest");
        assert!(crate::canonical::to_string(&q).unwrap().contains(r#""tier":"priority""#));
        assert_eq!(q.tier_or_class(), crate::tiers::Tier::Priority);
        // An older server's plan: the class says it.
        let mut old = p.clone();
        old.class = Some(crate::share::JobClass::Gate);
        assert_eq!(old.tier_or_class(), crate::tiers::Tier::Ci);
        old.class = Some(crate::share::JobClass::Batch);
        assert_eq!(old.tier_or_class(), crate::tiers::Tier::Normal);
        // An old node reads a tiered plan (no deny_unknown_fields) but gets
        // another digest: it refuses at assign, never runs it wrong.
        let mut v = serde_json::to_value(&q).unwrap();
        v.as_object_mut().unwrap().remove("tier");
        let dropped: Plan = serde_json::from_value(v).unwrap();
        assert_ne!(dropped.digest().unwrap(), q.digest().unwrap());
        assert!(PROTOCOL >= PROTOCOL_TIERS && PROTOCOL_TIERS > PROTOCOL_SECRETS);
    }

    /// Protocol 6: a plan without a cache is byte-identical to a 0.46.5
    /// plan (same digest as the tier test pins); a cache is bound by the
    /// digest, so an older node that drops the field refuses the assign
    /// instead of running the job without its cache.
    #[test]
    fn cache_is_absent_when_unset_and_bound_by_the_digest() {
        let p = plan();
        assert!(!crate::canonical::to_string(&p).unwrap().contains("cache"));
        assert_eq!(p.digest().unwrap(), "sha256:31d079662290d8b6e10bc3595162f277743d060a50046b1be2f020e99d948e14");
        let mut q = p.clone();
        q.cache = Some("fuzz".into());
        assert!(crate::canonical::to_string(&q).unwrap().contains(r#""cache":"fuzz""#));
        assert_ne!(q.digest().unwrap(), p.digest().unwrap());
        let mut v = serde_json::to_value(&q).unwrap();
        v.as_object_mut().unwrap().remove("cache");
        let dropped: Plan = serde_json::from_value(v).unwrap();
        assert_ne!(dropped.digest().unwrap(), q.digest().unwrap());
        assert!(PROTOCOL >= PROTOCOL_CACHE && PROTOCOL_CACHE > PROTOCOL_TIERS);
    }

    /// `--with`: absent when unused (the 0.46.5 digest), bound when used.
    #[test]
    fn with_is_absent_when_unset_and_bound_by_the_digest() {
        let p = plan();
        let text = crate::canonical::to_string(&p).unwrap();
        assert!(!text.contains("src_path") && !text.contains("\"with\""), "{text}");
        assert_eq!(p.digest().unwrap(), "sha256:31d079662290d8b6e10bc3595162f277743d060a50046b1be2f020e99d948e14");
        let mut q = p.clone();
        q.src_path = Some("k2-multiplayer-markdown".into());
        q.with = vec![SrcWith { path: "k2-multiplayer-base".into(), project_key: "pk2".into(), remote_url: None, commit: "b".repeat(40) }];
        assert_ne!(q.digest().unwrap(), p.digest().unwrap());
    }

    #[test]
    fn with_layouts_never_overlap_or_escape() {
        let w = |p: &str| SrcWith { path: p.into(), project_key: "k".into(), remote_url: None, commit: "a".repeat(40) };
        assert!(check_layout("k2-multiplayer-markdown", &[w("k2-multiplayer-base")]).is_ok());
        assert!(check_layout("", &[w("vendor/dep")]).is_ok(), "inside the workspace's own repo");
        assert!(check_layout("a/b", &[w("a/c"), w("d")]).is_ok());
        for (main, bad) in [("m", "m"), ("m", "m/x"), ("m/x", "m"), ("m", "../x"), ("m", "/etc"), ("m", ".k2/x"), ("m", "a//b"), ("m", "./a"), ("m", "")] {
            assert!(check_layout(main, &[w(bad)]).is_err(), "{main} + {bad}");
        }
        assert!(check_layout("../m", &[w("x")]).is_err());
        assert!(check_layout("m", &[w("x"), w("x")]).is_err(), "twice");
        assert!(check_layout("m", &(0..=MAX_WITH).map(|i| w(&format!("r{i}"))).collect::<Vec<_>>()).is_err());
    }

    /// S3b review H1: with the main repo as the layout root, a `--with`
    /// repo at a path the main checkout has (crates/k2-daemon) would
    /// replace those files under a receipt that names the main tree.
    #[test]
    fn a_with_repo_never_replaces_the_main_checkouts_files() {
        let w = |p: &str| SrcWith { path: p.into(), project_key: "k".into(), remote_url: None, commit: "a".repeat(40) };
        let tracked = |p: &str| matches!(p, "crates/k2-daemon" | "crates" | "Cargo.toml");
        let e = check_with_in_main_checkout("", &[w("vendor/dep"), w("crates/k2-daemon")], tracked).unwrap_err();
        assert!(e.contains("\"crates/k2-daemon\" is inside the main checkout"), "{e}");
        assert!(check_with_in_main_checkout("", &[w("Cargo.toml")], tracked).is_err());
        assert!(check_with_in_main_checkout("", &[w("vendor/dep")], tracked).is_ok(), "a nested repo the main one doesn't have");
        assert!(check_with_in_main_checkout("m", &[w("crates/k2-daemon")], tracked).is_ok(), "siblings: check_layout's overlap rule covers them");
    }

    #[test]
    fn cache_frames_travel_their_way() {
        let k = SigningKey::generate().unwrap();
        let get = Frame::CacheGet(crate::cache::CacheGet { request_id: "r1".into(), workspace_id: Some("w".into()), clear: true, name: Some("fuzz".into()) });
        assert_eq!((get.direction(), get.kind()), (Direction::ControllerToNode, "cache_get"));
        let t = seal(&k, Direction::ControllerToNode, "s", 0, &get).unwrap();
        assert_eq!(open(k.spki_der(), Direction::ControllerToNode, "s", 0, &t).unwrap(), get);
        let view = Frame::CacheView(crate::cache::CacheView {
            request_id: "r1".into(),
            at: 5,
            caches: vec![crate::cache::CacheEntry { ws_key: "0123456789abcdef".into(), name: "fuzz".into(), bytes: 4096, used_at: 4, busy: true }],
            cleared: vec![],
            refused: None,
        });
        assert_eq!((view.direction(), view.kind()), (Direction::NodeToController, "cache_view"));
        assert!(seal(&k, Direction::ControllerToNode, "s", 0, &view).is_err());
        let t = seal(&k, Direction::NodeToController, "s", 0, &view).unwrap();
        assert_eq!(open(k.spki_der(), Direction::NodeToController, "s", 0, &t).unwrap(), view);
    }

    /// Protocol 9 (C0): the statement frame travels node → controller only;
    /// a v1 receipt frame is 0.46.9's (no `statement` key); a v2 receipt's
    /// statement is checked against the digest, key and epoch it names.
    #[test]
    fn statement_frames_and_receipt_v2_statement_check() {
        use crate::ci::{Isolation, RunFacts, Signed, Taint, Wall};
        use crate::statement::{KeyExposure, NodeStatement};
        let k = SigningKey::generate().unwrap();
        let doc = |epoch: u32| NodeStatement {
            v: 1,
            node_fp: k.fingerprint(),
            node_spki: crypto::b64(k.spki_der()),
            key_epoch: epoch,
            key_born_at: 10,
            prev_fp: (epoch > 0).then(|| "a".repeat(64)),
            issued_at: 20,
            wall: Wall::JobUser,
            dev: false,
            taint: Taint::Clean,
            key_exposure: KeyExposure { store: crate::ci::KeyStore::File, owner: "k2node".into(), mode: Some(0o600), job_readable: false },
            k2node_version: "0.46.10".into(),
            k2node_commit: None,
            protocol: PROTOCOL,
            os: "linux".into(),
            arch: "x86_64".into(),
            distro: None,
            distro_version: None,
            kernel: None,
            ptrace_scope: Some(1),
            host_id: None,
        };
        let st = doc(0).sign(&k).unwrap();
        let f = Frame::NodeStatement(st.clone());
        assert_eq!((f.direction(), f.kind()), (Direction::NodeToController, "node_statement"));
        assert!(seal(&k, Direction::ControllerToNode, "s", 0, &f).is_err());
        let t = seal(&k, Direction::NodeToController, "s", 0, &f).unwrap();
        assert_eq!(open(k.spki_der(), Direction::NodeToController, "s", 0, &t).unwrap(), f);
        // A v1 receipt frame: no `statement` key at all.
        let sr = SignedReceipt { body: "{}".into(), sig: "x".into() };
        let rf = ReceiptFrame { job_id: "j".into(), generation: 1, receipt: sr.clone(), statement: None };
        assert_eq!(serde_json::to_string(&rf).unwrap(), r#"{"job_id":"j","generation":1,"receipt":{"body":"{}","sig":"x"}}"#);
        // A 0.46.9 frame (no key) still parses.
        let old: ReceiptFrame = serde_json::from_str(r#"{"job_id":"j","generation":1,"receipt":{"body":"{}","sig":"x"}}"#).unwrap();
        assert_eq!(old.statement, None);
        let run = RunFacts {
            isolation: Isolation::JobUid,
            uid_index: Some(2),
            wall: Wall::JobUser,
            tenant_slot: 1,
            k2node_version: "0.46.10".into(),
            k2node_commit: None,
            key_epoch: 0,
            statement_digest: st.digest(),
            roster_seq_seen: None,
            lane: None,
        };
        let r = Receipt {
            v: RECEIPT_V2,
            job_id: "j".into(),
            attempt: 1,
            generation: 1,
            plan_digest: "sha256:x".into(),
            node_fp: k.fingerprint(),
            controller_fp: "c".into(),
            workspace_id: "w".into(),
            argv_sha256: argv_sha256(&["true".into()]),
            env_names: vec![],
            src: None,
            tools: BTreeMap::new(),
            boot_id: "b".into(),
            started_at: Some(1),
            ended_at: 2,
            state: JobState::Done,
            exit: None,
            log: ReceiptLog { sha256: "s".into(), bytes: 0, truncated: false },
            artifacts: Vec::new(),
            secret_env: Vec::new(),
            tools_used: BTreeMap::new(),
            run: Some(run.clone()),
        };
        r.check_version().unwrap();
        assert_eq!(r.check_statement(&st).unwrap(), doc(0));
        // Another statement (the digest differs), a doctored one, another
        // key's, another epoch's: refused.
        let st1 = doc(1).sign(&k).unwrap();
        assert!(r.check_statement(&st1).unwrap_err().contains("statement_digest"));
        let forged = Signed { body: st.body.replace("\"dev\":false", "\"dev\":true"), sig: st.sig.clone() };
        assert!(r.check_statement(&forged).is_err());
        let other = SigningKey::generate().unwrap();
        let theirs = NodeStatement { node_fp: other.fingerprint(), node_spki: crypto::b64(other.spki_der()), ..doc(0) }.sign(&other).unwrap();
        let r2 = Receipt { run: Some(RunFacts { statement_digest: theirs.digest(), ..run.clone() }), ..r.clone() };
        assert!(r2.check_statement(&theirs).unwrap_err().contains("another node key"));
        let r3 = Receipt { run: Some(RunFacts { key_epoch: 1, statement_digest: st.digest(), ..run }), ..r.clone() };
        assert!(r3.check_statement(&st).unwrap_err().contains("epoch"));
        let v1 = Receipt { v: RECEIPT_V1, run: None, ..r };
        assert!(v1.check_statement(&st).is_err());
    }
}
