//! The JSON half of the call key wire (prd-sew-50 §9.1–§9.4, §14.3, §20; prd-sew-51 §12):
//! the key join (K1, K2), the KeyPackage upload (K16), the call routes K3–K10, the
//! `call_keys_delivered` dispatch body ([`CallKeysMessage`]), the DS-signed [`LeafBindingTable`],
//! the relayed recorder [`Release`] and the READY section ([`ReadyCallKeys`]).
//!
//! Requests refuse unknown fields; responses, records and dispatch bodies ignore them (C15).
//! Units: epochs `u64`, slots `u8`, leaves `u32`, ids [`Id`], `*_ms` `u64`. MLS objects are
//! [`B64`] (base64url, no padding). Sizes, quotas and every MLS check are the server's (S50
//! §8.4, §11): validators here bound strings and list lengths only.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::CallScope;
use super::b64::B64;
use super::names::{CallId, JoinIdHex, LkIdentity};
use super::tls::{self, BindingKey, LeafBindingEntry, LeafBindingTableTbs};
use crate::error::{Validate, ValidationError, check_known};
use crate::id::Id;

/// The only MLS ciphersuite of v1: `MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519` (§9.6).
pub const CIPHERSUITE_V1: u16 = 0x0001;
/// The only `kdf` value.
pub const KDF_HKDF: &str = "hkdf";
/// A `key_package_ref` is a 32-byte RFC 9420 `KeyPackageRef`.
pub const KEY_PACKAGE_REF_BYTES: usize = 32;
/// A `group_id` is 16 random bytes.
pub const GROUP_ID_BYTES: usize = 16;
/// Longest caps string (`sdk`, `sdk_version`, `browser`, `lib`).
pub const CAPS_STRING_MAX: usize = 64;
/// Most entries in `caps.ciphersuites`.
pub const CAPS_CIPHERSUITES_MAX: usize = 16;
/// Longest `gateway_session` (the READY `session_id` wire form).
pub const GATEWAY_SESSION_MAX: usize = 128;
/// KeyPackages per K16 upload.
pub const UPLOAD_KEY_PACKAGES_MAX: usize = 2;
/// The K10 `details` cap, as serialized JSON.
pub const REPORT_DETAILS_MAX_BYTES: usize = 256;

fn check_len(field: &str, s: &str, min: usize, max: usize) -> Result<(), ValidationError> {
    if s.len() < min || s.len() > max {
        return Err(ValidationError::invalid(
            field,
            format!("{field} must be {min}–{max} bytes, got {}", s.len()),
        ));
    }
    Ok(())
}

fn check_bytes(field: &str, b: &B64, len: usize) -> Result<(), ValidationError> {
    if b.len() != len {
        return Err(ValidationError::invalid(
            field,
            format!("{field} must be {len} bytes, got {}", b.len()),
        ));
    }
    Ok(())
}

fn check_nonempty(field: &str, b: &B64) -> Result<(), ValidationError> {
    if b.is_empty() {
        return Err(ValidationError::invalid(
            field,
            format!("{field} must not be empty"),
        ));
    }
    Ok(())
}

fn is_false(b: &bool) -> bool {
    !*b
}

// ---------------------------------------------------------------- caps and joins

/// What the joining client can do (S50 §9.1 K1, S51 §16 item 4). The server refuses an
/// incapable client with `422 e2ee_unsupported_client` (§11.3 J1), so the values themselves are
/// not checked here, only their sizes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct E2eeCaps {
    /// The SDK can move its sender to a new key slot.
    pub sender_key_index: bool,
    /// `"hkdf"`.
    pub kdf: String,
    /// 64 (rev 75); anything else is refused at join.
    pub key_ring_size: u32,
    /// The SDK family (`js`, `rust`, …).
    pub sdk: String,
    /// The SDK version as its `ClientInfo` reports it.
    pub sdk_version: String,
    /// `chrome`, `firefox`, `safari` or `other` (web only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<String>,
    /// `sew-e2ee/<version>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lib: Option<String>,
    /// MLS ciphersuites the client supports (S51 §6.1.1); must contain 1 when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ciphersuites: Option<Vec<u16>>,
}

impl Validate for E2eeCaps {
    fn validate(&self) -> Result<(), ValidationError> {
        check_len("kdf", &self.kdf, 1, CAPS_STRING_MAX)?;
        check_len("sdk", &self.sdk, 1, CAPS_STRING_MAX)?;
        check_len("sdk_version", &self.sdk_version, 1, CAPS_STRING_MAX)?;
        if let Some(b) = &self.browser {
            check_len("browser", b, 1, CAPS_STRING_MAX)?;
        }
        if let Some(l) = &self.lib {
            check_len("lib", l, 1, CAPS_STRING_MAX)?;
        }
        if let Some(c) = &self.ciphersuites
            && (c.is_empty() || c.len() > CAPS_CIPHERSUITES_MAX)
        {
            return Err(ValidationError::invalid(
                "ciphersuites",
                format!("ciphersuites must hold 1–{CAPS_CIPHERSUITES_MAX} values"),
            ));
        }
        Ok(())
    }
}

/// The `e2ee` object of a voice or call join (K1; S33 v2 `VoiceJoinRequest.e2ee`, S34 v2's call
/// joins).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct E2eeJoin {
    /// The single-use ref K16 returned (32 bytes).
    pub key_package_ref: B64,
    /// The caller's gateway session: READY's `session_id`.
    pub gateway_session: String,
    /// What the client can do.
    pub caps: E2eeCaps,
}

impl Validate for E2eeJoin {
    fn validate(&self) -> Result<(), ValidationError> {
        check_bytes(
            "key_package_ref",
            &self.key_package_ref,
            KEY_PACKAGE_REF_BYTES,
        )?;
        check_len(
            "gateway_session",
            &self.gateway_session,
            1,
            GATEWAY_SESSION_MAX,
        )?;
        self.caps.validate().map_err(|e| e.nested("caps"))
    }
}

/// K2 body: the key join after a move, with the Sew-minted `join_id` from
/// `voice_server_updated` (S50 §9.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyJoinRequest {
    /// The join id Sew minted for the move.
    pub join_id: JoinIdHex,
    /// The caller's gateway session.
    pub gateway_session: String,
    /// A ref uploaded for the target channel's scope.
    pub key_package_ref: B64,
    /// What the client can do.
    pub caps: E2eeCaps,
}

impl Validate for KeyJoinRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_bytes(
            "key_package_ref",
            &self.key_package_ref,
            KEY_PACKAGE_REF_BYTES,
        )?;
        check_len(
            "gateway_session",
            &self.gateway_session,
            1,
            GATEWAY_SESSION_MAX,
        )?;
        self.caps.validate().map_err(|e| e.nested("caps"))
    }
}

/// The client timings and constants the server relays (READY `call_keys.params` and every
/// [`CallKeysInit`]; S50 §9.4, S51 §20.1). The single source for client timings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CallKeysParams {
    /// The ciphersuite every group uses (1).
    pub ciphersuite: u16,
    /// The ciphersuites the server accepts (`[1]` in v1).
    pub ciphersuites: Vec<u16>,
    /// Always 64 (rev 75).
    pub key_ring_size: u32,
    /// `"hkdf"`.
    pub kdf: String,
    /// The MLS exporter label of frame keys ([`tls::EXPORTER_LABEL`]).
    pub exporter_label: String,
    /// The recorder release `info` label ([`tls::RELEASE_INFO_LABEL`]).
    pub release_info_label: String,
    /// Receivers retire an old slot this long after `activate`.
    pub retire_after_ms: u64,
    /// A sender that acked but hears no `activate` switches after this long.
    pub client_activate_fallback_ms: u64,
    /// Longest KeyPackage lifetime the server accepts.
    pub key_package_max_lifetime_s: u64,
    /// Dispatches for a `call_id` the client does not know yet are buffered this long.
    pub unknown_call_buffer_ms: u64,
    /// "Securing…" shows only after this long.
    pub securing_indicator_after_ms: u64,
}

/// What a key join answers (K1's `VoiceJoinResponse.e2ee`, K2, and the `init` dispatch;
/// S50 §9.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CallKeysInit {
    /// The call: its `r_…` room.
    pub call_id: CallId,
    /// The caller's Sew-minted join id; every K3–K10 body carries it.
    pub join_id: JoinIdHex,
    /// The caller's engine identity (`p_` + base32 of `join_id`; the token's `sub`).
    pub lk_identity: LkIdentity,
    /// The call's scope, `call:<channel_id>`; members refuse leaves with another.
    pub scope: CallScope,
    /// The current group (16 bytes).
    pub group_id: B64,
    /// The group's epoch.
    pub epoch: u64,
    /// The caller creates the group now (S50 §12.1).
    pub create: bool,
    /// The creator activates epoch 0 at once (first joiner of a fresh call only).
    pub activate_on_create: bool,
    /// `slot(e) = (slot_base + e) mod 64`.
    pub slot_base: u8,
    /// TLS `ExternalSender` of the install's DS key.
    pub external_sender: B64,
    /// Client timings.
    pub params: CallKeysParams,
}

// ---------------------------------------------------------------- K16 upload

/// K16 body: 1–2 KeyPackages uploaded before any join (rev 78 D2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyPackageUploadRequest {
    /// TLS `MLSMessage(KeyPackage)` each.
    pub key_packages: Vec<B64>,
}

impl Validate for KeyPackageUploadRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.key_packages.is_empty() || self.key_packages.len() > UPLOAD_KEY_PACKAGES_MAX {
            return Err(ValidationError::invalid(
                "key_packages",
                format!("key_packages must hold 1–{UPLOAD_KEY_PACKAGES_MAX} packages"),
            ));
        }
        for (i, kp) in self.key_packages.iter().enumerate() {
            check_nonempty(&format!("key_packages[{i}]"), kp)?;
        }
        Ok(())
    }
}

/// One uploaded KeyPackage's single-use ref.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KeyPackageRefItem {
    /// RFC 9420 `KeyPackageRef` (`KeyPackage::hash_ref`, 32 bytes).
    pub key_package_ref: B64,
    /// When the ref expires (unix ms).
    pub expires_at_ms: u64,
}

/// K16 answer: one item per package, in request order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KeyPackageUploadResponse {
    /// The refs.
    pub items: Vec<KeyPackageRefItem>,
}

// ---------------------------------------------------------------- binding tables

/// One `LeafBinding` entry in JSON: exactly one of `leaf` (key type 1) or `key_package_ref`
/// (key type 2), the join id and its engine identity (S50 §9.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LeafBindingEntryJson {
    /// The leaf index (commit, welcome, K3 reply, recorder tables).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leaf: Option<u32>,
    /// The Add's `key_package_ref` (proposals).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_package_ref: Option<B64>,
    /// The join id.
    pub join_id: JoinIdHex,
    /// `p_` + base32 of `join_id` (not signed; clients recompute and compare).
    pub lk_identity: LkIdentity,
}

/// The DS-signed leaf ↔ join id table (S50 §9.5, S51 §7.6). `sig` = Ed25519(`sew-call-ds`,
/// `"sew call leaf binding v1"` ‖ TLS([`LeafBindingTableTbs`])).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LeafBindingTable {
    /// The call.
    pub call_id: CallId,
    /// The group (16 bytes).
    pub group_id: B64,
    /// The proposal epoch (key type 2) or the new epoch (key type 1).
    pub epoch: u64,
    /// Ascending by leaf or by ref; one key type per table.
    pub entries: Vec<LeafBindingEntryJson>,
    /// 64-byte Ed25519 signature.
    pub sig: B64,
}

/// Why a JSON binding table does not match its signed form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingError {
    /// An entry has both or neither of `leaf` and `key_package_ref` (its index is named).
    EntryKey(usize),
    /// An entry's `lk_identity` is not `p_` + base32 of its `join_id` (its index is named).
    Identity(usize),
    /// The TBS `call_id` is not an `r_…` room.
    CallId,
    /// The content rules ([`LeafBindingTableTbs::check`]).
    Table(tls::TableError),
}

impl std::fmt::Display for BindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BindingError::EntryKey(i) => {
                write!(f, "entry {i} needs exactly one of leaf and key_package_ref")
            }
            BindingError::Identity(i) => write!(f, "entry {i}: lk_identity does not match join_id"),
            BindingError::CallId => f.write_str("call_id is not a room name"),
            BindingError::Table(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for BindingError {}

impl LeafBindingTable {
    /// The signed structure: entries in their key form, checked against the content rules
    /// (one key type, strictly ascending, unique join ids) and each `lk_identity`.
    pub fn tbs(&self) -> Result<LeafBindingTableTbs, BindingError> {
        let mut entries = Vec::with_capacity(self.entries.len());
        for (i, e) in self.entries.iter().enumerate() {
            let key = match (e.leaf, &e.key_package_ref) {
                (Some(l), None) => BindingKey::Leaf(l),
                (None, Some(r)) => BindingKey::KeyPackageRef(r.as_bytes().to_vec()),
                _ => return Err(BindingError::EntryKey(i)),
            };
            if e.join_id.lk_identity() != e.lk_identity {
                return Err(BindingError::Identity(i));
            }
            entries.push(LeafBindingEntry {
                key,
                join_id: e.join_id.bytes(),
            });
        }
        let tbs = LeafBindingTableTbs {
            call_id: self.call_id.as_str().as_bytes().to_vec(),
            group_id: self.group_id.as_bytes().to_vec(),
            epoch: self.epoch,
            entries,
        };
        tbs.check().map_err(BindingError::Table)?;
        Ok(tbs)
    }

    /// The JSON form of a signed structure (identities derived from the join ids).
    pub fn from_tbs(
        tbs: &LeafBindingTableTbs,
        sig: Vec<u8>,
    ) -> Result<LeafBindingTable, BindingError> {
        tbs.check().map_err(BindingError::Table)?;
        let call_id = std::str::from_utf8(&tbs.call_id)
            .ok()
            .and_then(|s| CallId::parse(s).ok())
            .ok_or(BindingError::CallId)?;
        let entries = tbs
            .entries
            .iter()
            .map(|e| {
                let join_id = JoinIdHex::from_bytes(e.join_id);
                let (leaf, key_package_ref) = match &e.key {
                    BindingKey::Leaf(l) => (Some(*l), None),
                    BindingKey::KeyPackageRef(r) => (None, Some(B64::new(r.clone()))),
                };
                LeafBindingEntryJson {
                    leaf,
                    key_package_ref,
                    lk_identity: join_id.lk_identity(),
                    join_id,
                }
            })
            .collect();
        Ok(LeafBindingTable {
            call_id,
            group_id: B64::new(tbs.group_id.clone()),
            epoch: tbs.epoch,
            entries,
            sig: B64::new(sig),
        })
    }
}

/// A full key-type-1 table of one committed epoch for the recorder (rev 88 D5;
/// `RecorderSink::bindings`, S35 v2 → S53 `/bindings`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BindingsUpdate {
    /// The group.
    pub group_id: B64,
    /// The epoch the table is for.
    pub epoch: u64,
    /// The group's slot base.
    pub slot_base: u8,
    /// The signed table.
    pub table: LeafBindingTable,
}

/// A public key and its id (`kid` = hex of the first 8 bytes of SHA-256 of the raw key).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CallPublicKey {
    /// 16 lowercase hex characters.
    pub kid: String,
    /// The raw 32-byte Ed25519 public key.
    pub public_key: B64,
}

/// The install CA and DS public keys (`CallKeyService::public_keys`, for S35's recorder PUT).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CallPublicKeys {
    /// `sew-call-ca`.
    pub ca: CallPublicKey,
    /// `sew-call-ds`.
    pub ds: CallPublicKey,
}

// ---------------------------------------------------------------- K3–K10

unit_enum! {
    /// K3's answer (S50 §9.1).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum KeyStateStatus {
        /// The leaf is bound and current.
        InSync = "in_sync",
        /// Repeat the call with `ratchet_tree`.
        SendTree = "send_tree",
        /// A rebuild is collecting reports; retry after `retry_after_ms`.
        Rebuilding = "rebuilding",
    }
}

/// K3 body: the create report, a re-bind after a re-identify, or a rebuild report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyStateRequest {
    /// The caller's join id.
    pub join_id: JoinIdHex,
    /// The caller's current gateway session.
    pub gateway_session: String,
    /// The group the client holds.
    pub group_id: B64,
    /// The reporter's own leaf index.
    pub leaf: u32,
    /// The client's epoch.
    pub epoch: u64,
    /// The client's active epoch.
    pub active_epoch: u64,
    /// The client's slot base.
    pub slot_base: u8,
    /// The client's tree hash.
    pub tree_hash: B64,
    /// SHA-256 of the TLS binding table the client holds.
    pub bindings_hash: B64,
    /// TLS `MLSMessage(GroupInfo)`.
    pub group_info: B64,
    /// TLS `RatchetTree`, only when the DS asked (`send_tree`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratchet_tree: Option<B64>,
}

impl Validate for KeyStateRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_len(
            "gateway_session",
            &self.gateway_session,
            1,
            GATEWAY_SESSION_MAX,
        )?;
        check_bytes("group_id", &self.group_id, GROUP_ID_BYTES)?;
        check_nonempty("tree_hash", &self.tree_hash)?;
        check_bytes("bindings_hash", &self.bindings_hash, 32)?;
        check_nonempty("group_info", &self.group_info)?;
        if let Some(t) = &self.ratchet_tree {
            check_nonempty("ratchet_tree", t)?;
        }
        Ok(())
    }
}

/// K3 answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KeyStateResponse {
    /// `in_sync`, `send_tree` or `rebuilding`.
    pub status: KeyStateStatus,
    /// With `rebuilding`: the rest of the rebuild window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    /// The full signed table when the client's `bindings_hash` differs from the DS's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bindings: Option<LeafBindingTable>,
}

/// K4 body: the designated committer's commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyCommitRequest {
    /// The committer's join id.
    pub join_id: JoinIdHex,
    /// The group.
    pub group_id: B64,
    /// The epoch the commit is for (the proposals' epoch).
    pub epoch: u64,
    /// TLS `MLSMessage(PublicMessage commit)`.
    pub commit: B64,
    /// TLS `MLSMessage(Welcome)`; present iff the commit adds leaves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub welcome: Option<B64>,
    /// TLS `MLSMessage(GroupInfo)` for `epoch + 1`.
    pub group_info: B64,
}

impl Validate for KeyCommitRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_bytes("group_id", &self.group_id, GROUP_ID_BYTES)?;
        check_nonempty("commit", &self.commit)?;
        if let Some(w) = &self.welcome {
            check_nonempty("welcome", w)?;
        }
        check_nonempty("group_info", &self.group_info)
    }
}

/// K4 `200` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KeyCommitAccepted {
    /// Always `true`.
    pub accepted: bool,
    /// The new epoch.
    pub epoch: u64,
    /// Its slot.
    pub slot: u8,
}

/// K5 body: every remote sender's key for `epoch` is installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyReadyRequest {
    /// The caller's join id.
    pub join_id: JoinIdHex,
    /// The group.
    pub group_id: B64,
    /// The installed epoch.
    pub epoch: u64,
}

impl Validate for KeyReadyRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_bytes("group_id", &self.group_id, GROUP_ID_BYTES)
    }
}

unit_enum! {
    /// Why a client asks to be re-added (K6; S50 §9.1, S51 §12.1).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum InvalidReason {
        /// A commit failed to process.
        CommitFailed = "commit_failed",
        /// A commit failed the member-side checks.
        BadCommit = "bad_commit",
        /// A proposal failed the member-side checks.
        BadProposal = "bad_proposal",
        /// A Welcome failed to process or check.
        BadWelcome = "bad_welcome",
        /// An epoch was missed.
        MissedEpoch = "missed_epoch",
        /// The recovery watchdog ran out.
        RecoveryTimeout = "recovery_timeout",
        /// K3 answered `e2ee_epoch_behind`, or a `resync{behind}`.
        Behind = "behind",
        /// The client lost its MLS state.
        StateLost = "state_lost",
    }
}

/// K6 body: re-add the caller's leaf with a fresh inline KeyPackage (one Replace commit).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyInvalidRequest {
    /// The caller's join id (kept by the new leaf).
    pub join_id: JoinIdHex,
    /// The group the client holds.
    pub group_id: B64,
    /// The client's epoch.
    pub epoch: u64,
    /// Why.
    pub reason: InvalidReason,
    /// TLS `MLSMessage(KeyPackage)`: same member, device and scope as the leaf.
    pub key_package: B64,
    /// The caller's current gateway session, when it re-identified while holding no group
    /// (its Welcome went to the dead socket, or its join waited across a restart): the DS
    /// re-binds the join to it (§15.3; additive, KB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_session: Option<String>,
}

impl Validate for KeyInvalidRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_bytes("group_id", &self.group_id, GROUP_ID_BYTES)?;
        check_known("reason", self.reason)?;
        if let Some(s) = &self.gateway_session {
            check_len("gateway_session", s, 1, GATEWAY_SESSION_MAX)?;
        }
        check_nonempty("key_package", &self.key_package)
    }
}

/// K7 body: a fresh inline KeyPackage for a resetting call's new group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyResetPackageRequest {
    /// The caller's join id.
    pub join_id: JoinIdHex,
    /// The group being collected (from `reset`).
    pub new_group_id: B64,
    /// TLS `MLSMessage(KeyPackage)`.
    pub key_package: B64,
}

impl Validate for KeyResetPackageRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_bytes("new_group_id", &self.new_group_id, GROUP_ID_BYTES)?;
        check_nonempty("key_package", &self.key_package)
    }
}

/// K8 body: the caller's own sender key for one epoch, sealed to the recorder (S50 §14.3).
/// `enc` and `ct` sizes and `sender_identity` are the DS's routing checks (`422`), not shape
/// checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyReleaseRequest {
    /// The caller's join id.
    pub join_id: JoinIdHex,
    /// The group.
    pub group_id: B64,
    /// The released epoch.
    pub epoch: u64,
    /// `slot(epoch)`.
    pub slot: u8,
    /// The live recording.
    pub recording_id: Id,
    /// The caller's leaf index in `epoch`.
    pub leaf: u32,
    /// The caller's engine identity.
    pub sender_identity: String,
    /// HPKE `enc` (32 bytes).
    pub enc: B64,
    /// HPKE ciphertext (48 bytes).
    pub ct: B64,
}

impl Validate for KeyReleaseRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_bytes("group_id", &self.group_id, GROUP_ID_BYTES)?;
        check_len("sender_identity", &self.sender_identity, 1, CAPS_STRING_MAX)
    }
}

/// A release as the DS relays it to the recorder (`RecorderSink::release`; S53 opens it with
/// S51's `ReleaseOpener`, which rebuilds the `info` with `leaf`; rev 88 D4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Release {
    /// The call.
    pub call_id: CallId,
    /// The recording.
    pub recording_id: Id,
    /// The group.
    pub group_id: B64,
    /// The epoch.
    pub epoch: u64,
    /// The sender's leaf index in `epoch`.
    pub leaf: u32,
    /// `slot(epoch)`.
    pub slot: u8,
    /// The sender's engine identity.
    pub sender_identity: LkIdentity,
    /// HPKE `enc` (32 bytes).
    pub enc: B64,
    /// HPKE ciphertext (48 bytes).
    pub ct: B64,
}

impl Release {
    /// The structure the HPKE `info` binds (§9.5).
    pub fn info(&self) -> tls::ReleaseInfo {
        tls::ReleaseInfo {
            call_id: self.call_id.as_str().as_bytes().to_vec(),
            recording_id: self.recording_id.get(),
            group_id: self.group_id.as_bytes().to_vec(),
            epoch: self.epoch,
            leaf_index: self.leaf,
            slot: self.slot,
            sender_identity: self.sender_identity.as_str().as_bytes().to_vec(),
        }
    }
}

unit_enum! {
    /// Why a client asks for an immediate rekey (K9).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum RekeyReason {
        /// The member withdrew recording consent (S50 §14.4).
        ConsentWithdrawn = "consent_withdrawn",
    }
}

/// K9 body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyRekeyRequest {
    /// The caller's join id.
    pub join_id: JoinIdHex,
    /// Why.
    pub reason: RekeyReason,
}

impl Validate for KeyRekeyRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_known("reason", self.reason)
    }
}

unit_enum! {
    /// What a K10 report is about (S50 §9.1, S51 §12.1).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ReportKind {
        /// A leaf whose member has no voice state in the channel.
        LeafWithoutVoiceState = "leaf_without_voice_state",
        /// Two leaves of one member outside a swap.
        DuplicateMember = "duplicate_member",
        /// An engine participant with no leaf past 5 s.
        EngineParticipantWithoutLeaf = "engine_participant_without_leaf",
        /// The group's external sender differs from READY's.
        ExternalSenderMismatch = "external_sender_mismatch",
        /// The CA key differs from the pinned one.
        CaChanged = "ca_changed",
        /// A binding table failed the client's checks.
        LeafBindingInvalid = "leaf_binding_invalid",
        /// A leaf's credential scope is empty or differs from the call's (S51 G35).
        ScopeInvalid = "scope_invalid",
        /// The reporter's own sender stays on a stale slot after one re-apply.
        SenderIndexStuck = "sender_index_stuck",
        /// Another leaf's frames do not decrypt (quorum only).
        PeerUndecryptable = "peer_undecryptable",
        /// A bridge saw an unencrypted track from the engine.
        EngineUnencryptedTrack = "engine_unencrypted_track",
        /// A bridge saw a non-empty `sif_trailer` from the engine.
        EngineSifTrailer = "engine_sif_trailer",
        /// A bridge saw an encryption downgrade from the engine.
        EngineEncryptionDowngrade = "engine_encryption_downgrade",
    }
}

impl ReportKind {
    /// The roster kinds (audited `call.e2ee_roster_report`, alarm `e2ee_roster_mismatch`).
    pub const fn is_roster(self) -> bool {
        matches!(
            self,
            ReportKind::LeafWithoutVoiceState
                | ReportKind::DuplicateMember
                | ReportKind::EngineParticipantWithoutLeaf
                | ReportKind::ExternalSenderMismatch
                | ReportKind::CaChanged
                | ReportKind::LeafBindingInvalid
                | ReportKind::ScopeInvalid
        )
    }

    /// The slot signals (`sender_index_stuck`, `peer_undecryptable`; their own count).
    pub const fn is_slot_signal(self) -> bool {
        matches!(
            self,
            ReportKind::SenderIndexStuck | ReportKind::PeerUndecryptable
        )
    }

    /// The bridge detections of a misbehaving engine.
    pub const fn is_engine(self) -> bool {
        matches!(
            self,
            ReportKind::EngineUnencryptedTrack
                | ReportKind::EngineSifTrailer
                | ReportKind::EngineEncryptionDowngrade
        )
    }
}

/// K10 `details` (≤ 256 bytes of JSON; S51 §13.6). Every field is optional; strings ≤ 64 bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReportDetails {
    /// The slot the sender should be on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_slot: Option<u8>,
    /// The slot the reporter saw.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_slot: Option<u8>,
    /// For how long the condition held.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_ms: Option<u64>,
    /// Failed decrypts counted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failures: Option<u32>,
    /// Re-apply attempts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempts: Option<u32>,
    /// The key index a receiver missed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_index: Option<u8>,
    /// A short reason (`missing_key`, `invalid_key`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The reporter's SDK family.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk: Option<String>,
    /// The reporter's SDK version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk_version: Option<String>,
}

impl Validate for ReportDetails {
    fn validate(&self) -> Result<(), ValidationError> {
        for (field, v) in [
            ("reason", &self.reason),
            ("sdk", &self.sdk),
            ("sdk_version", &self.sdk_version),
        ] {
            if let Some(s) = v {
                check_len(field, s, 1, CAPS_STRING_MAX)?;
            }
        }
        let n = serde_json::to_vec(self)
            .map(|v| v.len())
            .unwrap_or(usize::MAX);
        if n > REPORT_DETAILS_MAX_BYTES {
            return Err(ValidationError::too_large(
                "details",
                format!("details must be at most {REPORT_DETAILS_MAX_BYTES} bytes of JSON"),
            ));
        }
        Ok(())
    }
}

/// K10 body: a ghost-member, slot or engine report (S50 §16.4, §12.10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyReportRequest {
    /// The reporter's join id.
    pub join_id: JoinIdHex,
    /// The group the reporter holds.
    pub group_id: B64,
    /// The reporter's epoch.
    pub epoch: u64,
    /// What is reported.
    pub kind: ReportKind,
    /// The leaf it is about (the reporter's own for `sender_index_stuck`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leaf: Option<u32>,
    /// Optional details.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ReportDetails>,
}

impl Validate for KeyReportRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_bytes("group_id", &self.group_id, GROUP_ID_BYTES)?;
        check_known("kind", self.kind)?;
        if let Some(d) = &self.details {
            d.validate().map_err(|e| e.nested("details"))?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- the dispatch

unit_enum! {
    /// Why a transition started (`proposals.reason`; S50 §9.3).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum TransitionReason {
        /// Leaves joined.
        Join = "join",
        /// Leaves left or were removed.
        Leave = "leave",
        /// A device swap (Remove + Add of one member).
        Swap = "swap",
        /// A re-add after a missed epoch or a failed commit.
        Readd = "readd",
        /// A requested rekey.
        Rekey = "rekey",
        /// Recording consent was withdrawn.
        Consent = "consent",
        /// The recording stopped.
        RecorderStop = "recorder_stop",
        /// A device was revoked.
        Revoke = "revoke",
        /// The periodic rekey.
        Periodic = "periodic",
        /// A leaf-lifetime refresh.
        Refresh = "refresh",
        /// Leaves a rebuild could not match.
        RebuildOrphan = "rebuild_orphan",
        /// A sender stuck on a stale slot was removed.
        StuckSender = "stuck_sender",
    }
}

unit_enum! {
    /// Why a call's group is reset (`reset.reason`; S50 §12.9, §19.1).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ResetReason {
        /// Every committer candidate was silent.
        CommitterTimeout = "committer_timeout",
        /// A removal was not activated by `remove_deadline_ms`.
        RemoveDeadline = "remove_deadline",
        /// A rebuild found two equally covered groups.
        RebuildConflict = "rebuild_conflict",
        /// Too many invalid commits in one generation.
        InvalidCommits = "invalid_commits",
        /// A state report broke a group invariant.
        Invariant = "invariant",
    }
}

unit_enum! {
    /// Why a leaf is told to resync (`resync.reason`; S50 §9.3). `stale_slot` re-asserts the
    /// sender slot; every other reason is answered with K6.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum KeyResyncReason {
        /// The leaf is behind the DS's epoch.
        Behind = "behind",
        /// The leaf holds another group.
        GroupMismatch = "group_mismatch",
        /// The leaf stopped acking.
        Lagging = "lagging",
        /// The engine sees the leaf sending on the wrong slot.
        StaleSlot = "stale_slot",
    }
}

coded_enum! {
    /// Why a leaf left the key group (`removed.reason`; every S33 v2 §10.4 leave reason plus
    /// S50's and S34 v2's own). Codes are stable for metrics and logs.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum RemoveReason {
        /// The member left.
        Left = "left" => 1,
        /// The media connection ended for good.
        Disconnected = "disconnected" => 2,
        /// Another device of the member took over (swap).
        Replaced = "replaced" => 3,
        /// The member moved or switched channel.
        Moved = "moved" => 4,
        /// A moderator disconnected the member.
        RemovedByModerator = "removed_by_moderator" => 5,
        /// The member lost CONNECT or VIEW.
        PermsLost = "perms_lost" => 6,
        /// The channel was archived.
        ChannelArchived = "channel_archived" => 7,
        /// The member left or was removed from the space.
        RemovedFromSpace = "removed_from_space" => 8,
        /// The member was timed out.
        TimedOut = "timed_out" => 9,
        /// The account was deprovisioned.
        AccountRemoved = "account_removed" => 10,
        /// The login session was revoked.
        SessionRevoked = "session_revoked" => 11,
        /// The device was revoked.
        DeviceRevoked = "device_revoked" => 12,
        /// The media outlived every socket.
        Orphaned = "orphaned" => 13,
        /// Alone in the call too long.
        AloneTimeout = "alone_timeout" => 14,
        /// Media never connected.
        ConnectTimeout = "connect_timeout" => 15,
        /// The engine restarted and the media did not come back.
        MediaRestarted = "media_restarted" => 16,
        /// Voice was turned off.
        VoiceDisabled = "voice_disabled" => 17,
        /// The sender stayed on a stale key slot (rev 78 D4).
        E2eeSenderIndexStuck = "e2ee_sender_index_stuck" => 18,
        /// The device stopped answering key requests (S50 unresponsive removal, F16 #29).
        Unresponsive = "unresponsive" => 19,
        /// A rebuild found no live entry for the leaf.
        RebuildOrphan = "rebuild_orphan" => 20,
        /// The creator's group was refused.
        CreateFailed = "create_failed" => 21,
        /// The DM call ended for everyone (S34 v2, rev 88 D9).
        CallEnded = "call_ended" => 22,
    }
}

impl RemoveReason {
    /// A forced removal: audited `call.e2ee_member_removed` (S50 §19.4). Ordinary departures
    /// are not.
    pub const fn is_forced(self) -> bool {
        !matches!(
            self,
            RemoveReason::Left
                | RemoveReason::Disconnected
                | RemoveReason::Replaced
                | RemoveReason::Moved
                | RemoveReason::ConnectTimeout
                | RemoveReason::CallEnded
        )
    }
}

/// The `call_keys_delivered` dispatch body, tagged by `kind` (S50 §9.3). Clients ignore kinds
/// they do not know ([`CallKeysMessage::Unknown`], C15).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CallKeysMessage {
    /// A creator failover or a reset creator: the [`CallKeysInit`] fields.
    Init(CallKeysInit),
    /// A transition starts.
    Proposals {
        /// The call.
        call_id: CallId,
        /// The group.
        group_id: B64,
        /// The epoch the proposals are for.
        epoch: u64,
        /// DS-signed external `MLSMessage`s (Add and Remove only); empty for a rekey.
        proposals: Vec<B64>,
        /// The designated committer's leaf.
        committer: u32,
        /// Why.
        reason: TransitionReason,
        /// One entry per Add, keyed by its `key_package_ref`.
        bindings: LeafBindingTable,
    },
    /// Failover to the next committer candidate.
    CommitRequest {
        /// The call.
        call_id: CallId,
        /// The group.
        group_id: B64,
        /// The epoch.
        epoch: u64,
        /// The candidate's leaf.
        committer: u32,
        /// 2 or 3.
        attempt: u32,
    },
    /// The accepted commit.
    Commit {
        /// The call.
        call_id: CallId,
        /// The group.
        group_id: B64,
        /// The epoch the commit leaves.
        epoch_from: u64,
        /// TLS `MLSMessage(PublicMessage commit)`.
        commit: B64,
        /// The slot of `epoch_from + 1`.
        slot: u8,
        /// The leaves this commit added, keyed by leaf, for `epoch_from + 1`.
        bindings: LeafBindingTable,
    },
    /// A joiner's Welcome.
    Welcome {
        /// The call.
        call_id: CallId,
        /// The group.
        group_id: B64,
        /// The joiner's join id.
        join_id: JoinIdHex,
        /// The epoch the Welcome joins.
        epoch: u64,
        /// TLS `MLSMessage(Welcome)`.
        welcome: B64,
        /// TLS `RatchetTree` of `epoch`.
        ratchet_tree: B64,
        /// The group's slot base.
        slot_base: u8,
        /// `slot(epoch)`.
        slot: u8,
        /// The full table of `epoch`.
        bindings: LeafBindingTable,
    },
    /// Every sender switches to `slot`.
    Activate {
        /// The call.
        call_id: CallId,
        /// The group.
        group_id: B64,
        /// The epoch.
        epoch: u64,
        /// Its slot.
        slot: u8,
    },
    /// The group is replaced; send a fresh KeyPackage (K7).
    Reset {
        /// The call.
        call_id: CallId,
        /// The group being collected.
        new_group_id: B64,
        /// Why.
        reason: ResetReason,
        /// The new group's slot base.
        slot_base: u8,
        /// The collection window.
        collect_ms: u64,
    },
    /// The recorder: a certified key recipient, or `removed: true` (no key fields).
    Recorder {
        /// The call.
        call_id: CallId,
        /// The recording.
        recording_id: Id,
        /// TLS `RecorderCert`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recorder_cert: Option<B64>,
        /// The recorder's X25519 public key (32 bytes).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hpke_pub: Option<B64>,
        /// The recorder's engine identity (`p_…`, from S33's `admit_recorder`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lk_identity: Option<LkIdentity>,
        /// The recording stopped.
        #[serde(default, skip_serializing_if = "is_false")]
        removed: bool,
    },
    /// Device certificates were revoked.
    Revoked {
        /// The revoked serials.
        serials: Vec<Id>,
    },
    /// The leaf was removed; its last key message.
    Removed {
        /// The call.
        call_id: CallId,
        /// The group.
        group_id: B64,
        /// The removed join.
        join_id: JoinIdHex,
        /// Why.
        reason: RemoveReason,
    },
    /// The dispatch form of K3's `409` answers, and `stale_slot`.
    Resync {
        /// The call.
        call_id: CallId,
        /// The leaf's join id.
        join_id: JoinIdHex,
        /// Why.
        reason: KeyResyncReason,
        /// With `stale_slot`: the slot the sender must use.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_slot: Option<u8>,
    },
    /// A kind this build does not know.
    #[serde(other)]
    #[schemars(skip)]
    Unknown,
}

impl CallKeysMessage {
    /// The `kind` string (`"unknown"` for [`CallKeysMessage::Unknown`]); metrics label it.
    pub const fn kind(&self) -> &'static str {
        match self {
            CallKeysMessage::Init(_) => "init",
            CallKeysMessage::Proposals { .. } => "proposals",
            CallKeysMessage::CommitRequest { .. } => "commit_request",
            CallKeysMessage::Commit { .. } => "commit",
            CallKeysMessage::Welcome { .. } => "welcome",
            CallKeysMessage::Activate { .. } => "activate",
            CallKeysMessage::Reset { .. } => "reset",
            CallKeysMessage::Recorder { .. } => "recorder",
            CallKeysMessage::Revoked { .. } => "revoked",
            CallKeysMessage::Removed { .. } => "removed",
            CallKeysMessage::Resync { .. } => "resync",
            CallKeysMessage::Unknown => "unknown",
        }
    }
}

// ---------------------------------------------------------------- READY, consent, status

unit_enum! {
    /// Policy `calls.verification_code` (S50 §16.4): whether "Call info → Security code" is
    /// shown. `required` is reserved for a later mode and refused by the policy validator.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum VerificationCode {
        /// Shown on request, never modal (the default).
        Shown = "shown",
        /// Not shown.
        Hidden = "hidden",
        /// Reserved: "confirm before you speak" (refused in v1).
        Required = "required",
    }
}

/// READY `call_keys` (S50 §9.4): the trust anchors and client parameters. Absent when voice is
/// disabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadyCallKeys {
    /// TLS `ExternalSender` of every group.
    pub external_sender: B64,
    /// The raw 32-byte DS Ed25519 key, for pinning.
    pub ds_public_key: B64,
    /// The install CA.
    pub ca: CallPublicKey,
    /// Serials of revoked, unexpired device certificates (at most 5,000).
    pub crl: Vec<Id>,
    /// More revoked serials exist: page them with `GET /api/v1/devices/crl`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub crl_partial: bool,
    /// Policy `calls.verification_code`.
    pub verification_code: VerificationCode,
    /// Client timings.
    pub params: CallKeysParams,
}

unit_enum! {
    /// A member's consent for one recording (S35 v2's `ConsentSource`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ConsentState {
        /// Granted: releases are forwarded.
        Granted = "granted",
        /// Declined.
        Denied = "denied",
        /// Not answered yet.
        Pending = "pending",
    }
}

/// `status.voice.e2ee` (S50 §19.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusVoiceE2ee {
    /// Live key groups.
    pub calls: u32,
    /// Leaves across them.
    pub leaves: u32,
    /// Lagging leaves.
    pub lagging: u32,
    /// Calls rebuilding after a restart.
    pub rebuilding: u32,
    /// Group resets in the last hour.
    pub resets_1h: u32,
    /// Removals past their hard bound in the last hour.
    pub removal_overdue_1h: u32,
    /// Roster-kind K10 reports in the last hour.
    pub roster_reports_1h: u32,
    /// The CA key id; absent when the key failed to load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_kid: Option<String>,
    /// The DS key id; absent when the key failed to load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ds_kid: Option<String>,
    /// Unrevoked enrolled devices.
    pub devices: u64,
    /// Serials in the in-memory CRL.
    pub crl_size: u64,
}
