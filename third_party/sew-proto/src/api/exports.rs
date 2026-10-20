//! S23: the compliance flag, eDiscovery exports and full exports (prd-sew-23 §6, §9, §11,
//! §14; contracts rev 37).
//!
//! Client routes live under `/api/v1/compliance/exports…` (compliance-flag holders) and
//! `/api/v1/install/members/{id}/compliance` (the install owner); mgmt twins under
//! `/mgmt/v1/…`. Bodies carry ids, names, counts and hashes, never content or keywords.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Validate, ValidationError, check_chars, check_known};
use crate::export::{ArchiveOptions, ArchiveScope, ExportKind, FileMode, MemberMode};
use crate::id::Id;
use crate::limits::PAGE_LIMIT_MAX;
use crate::time::Timestamp;

/// `details.reason` values of S23 errors.
pub mod reasons {
    /// `403 missing_permission`: only the install owner sets the compliance flag.
    pub const OWNER_REQUIRED: &str = "owner_required";
    /// `422 invalid_state`: guests never hold the compliance flag.
    pub const TARGET_IS_GUEST: &str = "target_is_guest";
    /// `422 invalid_state`: agents never hold the compliance flag.
    pub const TARGET_IS_AGENT: &str = "target_is_agent";
    /// `422 invalid_state`: the member is deactivated.
    pub const TARGET_INACTIVE: &str = "target_inactive";
    /// `403 missing_permission`: a space-level audit reader must name `space_id`.
    pub const AUDIT_SCOPE_REQUIRED: &str = "audit_scope_required";
    /// `400 validation_failed` on `recipients`: not an age X25519 recipient (`details.index`).
    pub const RECIPIENT_INVALID: &str = "recipient_invalid";
    /// `503 unavailable`: the estimate took longer than 5 s.
    pub const ESTIMATE_TIMEOUT: &str = "estimate_timeout";
    /// `422 invalid_state`: the export is not `done`.
    pub const EXPORT_NOT_READY: &str = "export_not_ready";
    /// `422 invalid_state`: only `queued` or `running` exports can be cancelled.
    pub const NOT_CANCELLABLE: &str = "not_cancellable";
    /// `422 invalid_state`: `queued` or `running` exports must be cancelled first.
    pub const NOT_DELETABLE: &str = "not_deletable";
    /// `Export.error.reason`: the requester's compliance flag was revoked.
    pub const REQUESTER_REVOKED: &str = "requester_revoked";
    /// `Export.error.reason`: a caller cancelled it.
    pub const CANCELLED: &str = "cancelled";
    /// `Export.error.reason`: not enough disk space.
    pub const STORAGE_FULL: &str = "storage_full";
    /// `Export.error.reason`: a value did not decrypt (other than a shredded key).
    pub const DECRYPT_FAILED: &str = "decrypt_failed";
    /// `Export.error.reason`: the job failed for another reason.
    pub const INTERNAL: &str = "internal";
    /// `403 quota_exceeded`: `details.limit` when 20 exports are queued.
    pub const EXPORTS_QUEUED: &str = "exports_queued";
}

/// Most ids one eDiscovery request names (members + channels + spaces + holds).
pub const EDISCOVERY_TARGETS_MAX: usize = 1_000;
/// Most keywords.
pub const EDISCOVERY_KEYWORDS_MAX: usize = 50;
/// Longest keyword (characters).
pub const EDISCOVERY_KEYWORD_MAX_CHARS: usize = 200;
/// Longest label (characters).
pub const EXPORT_LABEL_MAX_CHARS: usize = 100;

fn yes() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

fn check_ids(field: &str, ids: &[Id]) -> Result<(), ValidationError> {
    let mut seen = ids.to_vec();
    seen.sort_unstable();
    seen.dedup();
    if seen.len() != ids.len() {
        return Err(ValidationError::invalid(
            field,
            format!("{field} lists an id twice"),
        ));
    }
    Ok(())
}

fn check_limit(limit: Option<u32>) -> Result<(), ValidationError> {
    if let Some(l) = limit
        && !(1..=PAGE_LIMIT_MAX).contains(&l)
    {
        return Err(ValidationError::invalid(
            "limit",
            format!("limit must be 1–{PAGE_LIMIT_MAX}"),
        ));
    }
    Ok(())
}

/// Shape check of age recipients (1–16, `age1…`); the server parses each one and names the
/// bad one's index.
pub fn check_recipients(recipients: &[String]) -> Result<(), ValidationError> {
    if recipients.is_empty() || recipients.len() > crate::mgmt::EXPORT_RECIPIENTS_MAX {
        return Err(ValidationError::invalid(
            "recipients",
            format!(
                "recipients must hold 1–{} age X25519 recipients",
                crate::mgmt::EXPORT_RECIPIENTS_MAX
            ),
        ));
    }
    Ok(())
}

/// Body of `POST /api/v1/compliance/exports` and `…/exports/estimate` (where `recipients` may
/// be empty).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateEdiscoveryRequest {
    /// 1–100 characters of plaintext metadata. Never describe the case in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Member targets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<Id>,
    /// Channel targets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<Id>,
    /// Space targets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spaces: Vec<Id>,
    /// Legal holds whose targets and ranges join the scope (released holds allowed).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holds: Vec<Id>,
    /// Window start (inclusive) of the request's own selector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Timestamp>,
    /// Window end (exclusive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Timestamp>,
    /// How member targets select (default `conversations`).
    #[serde(default)]
    pub member_mode: MemberMode,
    /// 0–50 keywords, 1–200 characters each (NFKC, case-insensitive substrings).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    /// File bytes or metadata only (default `include`).
    #[serde(default)]
    pub files: FileMode,
    /// HTML and CSV views (default `true`).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub views: bool,
    /// The audit subset (default `true`).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub audit: bool,
    /// 1–16 age X25519 recipients (`age1…`). The estimate route needs none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recipients: Vec<String>,
}

impl CreateEdiscoveryRequest {
    /// Every rule except the recipients (the estimate route's rules).
    pub fn validate_scope(&self) -> Result<(), ValidationError> {
        if let Some(l) = &self.label {
            check_chars("label", l, 1, EXPORT_LABEL_MAX_CHARS)?;
        }
        check_ids("members", &self.members)?;
        check_ids("channels", &self.channels)?;
        check_ids("spaces", &self.spaces)?;
        check_ids("holds", &self.holds)?;
        let total = self.members.len() + self.channels.len() + self.spaces.len() + self.holds.len();
        if total == 0 {
            return Err(ValidationError::invalid(
                "members",
                "name at least one member, channel, space or hold",
            ));
        }
        if total > EDISCOVERY_TARGETS_MAX {
            return Err(ValidationError::invalid(
                "members",
                format!("at most {EDISCOVERY_TARGETS_MAX} ids in total"),
            ));
        }
        if let (Some(f), Some(t)) = (self.from, self.to)
            && f >= t
        {
            return Err(ValidationError::invalid("to", "from must be before to"));
        }
        if self.keywords.len() > EDISCOVERY_KEYWORDS_MAX {
            return Err(ValidationError::invalid(
                "keywords",
                format!("at most {EDISCOVERY_KEYWORDS_MAX} keywords"),
            ));
        }
        for k in &self.keywords {
            check_chars("keywords", k, 1, EDISCOVERY_KEYWORD_MAX_CHARS)?;
        }
        check_known("member_mode", self.member_mode)?;
        check_known("files", self.files)
    }

    /// The recorded options.
    pub fn options(&self) -> ArchiveOptions {
        ArchiveOptions {
            files: self.files,
            views: self.views,
            audit: self.audit,
        }
    }
}

impl Validate for CreateEdiscoveryRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.validate_scope()?;
        check_recipients(&self.recipients)
    }
}

unit_enum! {
    /// An export's lifecycle state.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum ExportState {
        /// Waiting for the one export slot.
        Queued = "queued",
        /// Being written.
        Running = "running",
        /// The archive is ready for download.
        Done = "done",
        /// It failed (`error` says why).
        Failed = "failed",
        /// Cancelled (by a caller, or because the requester lost the flag).
        Cancelled = "cancelled",
        /// The archive passed `export.keep_days` and was deleted.
        Expired = "expired",
        /// The archive was deleted by a caller.
        Deleted = "deleted",
    }
}

/// An export's progress, written at most every 2 s while it runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportProgress {
    /// The phase (`snapshot`, `entities`, `messages`, `files`, `views`, `audit`, `finish`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// Channels written.
    #[serde(default)]
    pub channels_done: u64,
    /// Channels to write.
    #[serde(default)]
    pub channels_total: u64,
    /// Messages written.
    #[serde(default)]
    pub messages: u64,
    /// Files written.
    #[serde(default)]
    pub files: u64,
    /// Ciphertext bytes written.
    #[serde(default)]
    pub bytes_written: u64,
}

/// Why an export failed or was cancelled (codes and reasons only, never content).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportError {
    /// An error code (`storage_full`, `internal`, `invalid_state`, …).
    pub code: String,
    /// The reason (`requester_revoked`, `storage_full`, `decrypt_failed`, …).
    pub reason: String,
}

unit_enum! {
    /// Who requested an export.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum RequestedVia {
        /// A compliance-flag holder through the client API.
        Member = "member",
        /// The mgmt service (K2's daemon, the `sew` CLI).
        Service = "service",
    }
}

/// An export (full or eDiscovery). `id` is also its job id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Export {
    /// The export id (= the `export` job id).
    pub id: Id,
    /// Full or eDiscovery.
    pub kind: ExportKind,
    /// The requester's label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// State.
    pub state: ExportState,
    /// The requesting member (absent for the mgmt service).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_by: Option<Id>,
    /// Member or service.
    pub requested_via: RequestedVia,
    /// The eDiscovery scope (ids, dates, keyword count).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<ArchiveScope>,
    /// Files, views, audit.
    pub options: ArchiveOptions,
    /// The age recipients (request plus operator escrow recipients).
    pub recipients: Vec<String>,
    /// Progress.
    pub progress: ExportProgress,
    /// The archive's size, once done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_bytes: Option<u64>,
    /// SHA-256 of the archive file (ciphertext), 64 hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_sha256: Option<String>,
    /// The trailer's root hash, 64 hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_hash: Option<String>,
    /// Records per entity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counts: Option<BTreeMap<String, u64>>,
    /// Why it failed or was cancelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ExportError>,
    /// Requested.
    pub created_at: Timestamp,
    /// Started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<Timestamp>,
    /// Finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<Timestamp>,
    /// When the archive is deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
}

/// Response of `GET …/compliance/exports`, `GET /mgmt/v1/exports`.
pub type ExportPage = crate::http::Page<Export>;

/// Query of `GET /api/v1/compliance/exports` and `GET /mgmt/v1/exports` (newest first).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportsQuery {
    /// Only this kind (mgmt; the client route lists eDiscovery exports only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ExportKind>,
    /// Only this state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<ExportState>,
    /// Exports older than this id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Id>,
    /// Page size, 1–100 (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl Validate for ExportsQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(k) = self.kind {
            check_known("kind", k)?;
        }
        if let Some(s) = self.state {
            check_known("state", s)?;
        }
        check_limit(self.limit)
    }
}

/// Response of `POST /api/v1/compliance/exports/estimate`: counts under the snapshot rules, no
/// decryption (so keywords are not applied).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportEstimate {
    /// Channels in scope.
    pub channels: u64,
    /// Messages in scope (before keywords and context roots).
    pub messages: u64,
    /// Attachments of those messages.
    pub attachments: u64,
    /// Their plaintext bytes.
    pub file_bytes: u64,
    /// Always `false`: keywords need decryption.
    pub keywords_applied: bool,
}

/// Response of `POST /api/v1/compliance/exports/{id}/link`: a 1 h signed download link bound to
/// the caller (it stops working when the caller loses the flag).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SignedLink {
    /// The link (relative to the client API).
    pub url: String,
    /// When it stops working.
    pub expires_at: Timestamp,
}

/// Body of `POST /api/v1/install/members/{id}/compliance`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetComplianceRequest {
    /// Grant (`true`) or revoke (`false`).
    pub compliance: bool,
}

impl Validate for SetComplianceRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Response of the compliance-flag routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ComplianceFlagResponse {
    /// The member.
    pub member_id: Id,
    /// The flag now.
    pub compliance: bool,
    /// Whether this call changed it.
    pub changed: bool,
}

/// One active compliance-flag holder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ComplianceOfficer {
    /// The member.
    pub member_id: Id,
    /// When the flag was set (absent for flags set before S23).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_at: Option<Timestamp>,
    /// Who set it (absent for the mgmt service).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_by: Option<Id>,
}

/// Response of `GET /api/v1/compliance/officers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ComplianceOfficers {
    /// Active holders, by member id.
    pub items: Vec<ComplianceOfficer>,
}

/// `status.exports` on `GET /mgmt/v1/status` (prd-sew-23 §16).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusExports {
    /// The running export.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<Id>,
    /// Queued exports.
    pub queued: u32,
    /// Bytes of archives on disk.
    pub on_disk_bytes: u64,
    /// The last export that finished `done`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_done_at: Option<Timestamp>,
    /// The last export that failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failed_at: Option<Timestamp>,
}
