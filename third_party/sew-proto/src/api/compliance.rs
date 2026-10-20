//! Compliance (S22): retention policies, previews, runs, legal holds, the compliance channel
//! list, and the `content_vanished` event (prd-sew-22 §16.3, contracts rev 33).
//!
//! Every route lives under `/api/v1/compliance/` (compliance-flag holders) and
//! `/mgmt/v1/compliance/` (the mgmt service). Bodies carry ids, names, values and counts only,
//! never content. Retention values are days; `null` means "keep forever" on the install row
//! and "inherit" on a space or channel row.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::channels::{ChannelType, Visibility};
use crate::error::{Validate, ValidationError, check_known};
use crate::http::{PageQuery, Patch};
use crate::id::Id;
use crate::limits::PAGE_LIMIT_MAX;
use crate::time::Timestamp;

/// Largest `messages_days` / `files_days` (100 years).
pub const RETENTION_DAYS_MAX: u32 = 36_500;
/// Largest `deleted_purge_days` (10 years).
pub const DELETED_PURGE_DAYS_MAX: u32 = 3_650;
/// Longest hold name (characters).
pub const HOLD_NAME_MAX_CHARS: usize = 100;
/// Most targets one hold names.
pub const HOLD_TARGETS_MAX: usize = 1_000;
/// Most unreleased holds per install.
pub const ACTIVE_HOLDS_MAX: usize = 100;

/// `details.reason` values of S22 errors.
pub mod reasons {
    /// `403 missing_permission`: the caller lacks the compliance flag (agents never hold it).
    pub const COMPLIANCE_REQUIRED: &str = "compliance_required";
    /// `400 validation_failed`: `deleted_purge_days` on a space or channel.
    pub const INSTALL_ONLY: &str = "install_only";
    /// `400 validation_failed`: an org-policy write naming a retention mirror key.
    pub const MANAGED_BY_RETENTION: &str = "managed_by_retention";
    /// `422`: a value longer than the parent's active effective value.
    pub const RETENTION_EXCEEDS_PARENT: &str = "retention_exceeds_parent";
    /// `422`: a preview's `effective_at` is less than `min_notice_hours` ahead.
    pub const NOTICE_TOO_SHORT: &str = "notice_too_short";
    /// `422`: a preview's `effective_at` is more than 365 days ahead.
    pub const NOTICE_TOO_LONG: &str = "notice_too_long";
    /// `422`: a shorter policy needs `set {preview_id}`.
    pub const PREVIEW_REQUIRED: &str = "preview_required";
    /// `422`: the preview has not finished.
    pub const PREVIEW_NOT_READY: &str = "preview_not_ready";
    /// `422`: the preview is older than `preview_ttl_hours`.
    pub const PREVIEW_EXPIRED: &str = "preview_expired";
    /// `422`: the preview was already used by a `set`.
    pub const PREVIEW_USED: &str = "preview_used";
    /// `422`: only a pending policy can be cancelled.
    pub const NOT_PENDING: &str = "not_pending";
    /// `422`: a hold update may only widen.
    pub const HOLD_NARROWING_FORBIDDEN: &str = "hold_narrowing_forbidden";
    /// `422`: the hold is released.
    pub const HOLD_RELEASED: &str = "hold_released";
    /// `422`: `confirm_name` differs from the hold's name.
    pub const CONFIRM_MISMATCH: &str = "confirm_mismatch";
    /// `503`: a hold write waited too long for a running purge batch; retry.
    pub const RETENTION_BUSY: &str = "retention_busy";
}

/// Which level a retention policy sets (`kind` tag).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RetentionScope {
    /// The whole install (DMs use it too). A struct variant so unknown fields are refused.
    Install {},
    /// One space and every channel in it.
    Space {
        /// The space.
        id: Id,
    },
    /// One channel, DM or group DM.
    Channel {
        /// The channel.
        id: Id,
    },
}

/// A set of retention values. `null` = keep forever (install) or inherit (space, channel).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetentionValues {
    /// Days messages live, 1–36,500.
    #[serde(default)]
    pub messages_days: Option<u32>,
    /// Days files live, 1–36,500 (never longer than messages in effect).
    #[serde(default)]
    pub files_days: Option<u32>,
    /// Install only: days deleted and edited-away content is kept, 1–3,650 (`null` = off).
    #[serde(default)]
    pub deleted_purge_days: Option<u32>,
}

fn check_days(field: &str, v: Option<u32>, max: u32) -> Result<(), ValidationError> {
    match v {
        Some(d) if !(1..=max).contains(&d) => Err(ValidationError::invalid(
            field,
            format!("{field} must be 1–{max} or null"),
        )),
        _ => Ok(()),
    }
}

impl RetentionValues {
    /// Bounds only (the install-only rule is the server's, with its reason).
    pub fn check_bounds(&self) -> Result<(), ValidationError> {
        check_days("messages_days", self.messages_days, RETENTION_DAYS_MAX)?;
        check_days("files_days", self.files_days, RETENTION_DAYS_MAX)?;
        check_days(
            "deleted_purge_days",
            self.deleted_purge_days,
            DELETED_PURGE_DAYS_MAX,
        )
    }
}

unit_enum! {
    /// A retention policy row's lifecycle state.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum PolicyState {
        /// Waiting for its `effective_at` (a shorter value after a preview).
        Pending = "pending",
        /// In effect.
        Active = "active",
        /// Replaced by a newer active row.
        Superseded = "superseded",
        /// Cancelled before it took effect.
        Cancelled = "cancelled",
    }
}

unit_enum! {
    /// Who wrote a retention policy row.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum CreatedVia {
        /// A compliance-flag holder through the client API.
        Member = "member",
        /// The mgmt service (K2's daemon, the `sew` CLI).
        Service = "service",
        /// Sew itself (the install seed).
        System = "system",
    }
}

unit_enum! {
    /// A preview's or run's state.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum RunState {
        /// Working.
        Running = "running",
        /// Finished.
        Done = "done",
        /// Gave up (a later run starts over).
        Failed = "failed",
    }
}

unit_enum! {
    /// What started a retention run.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum RunTrigger {
        /// The daily schedule (02:00 UTC).
        Schedule = "schedule",
        /// `POST …/retention/run`.
        Manual = "manual",
        /// A pending policy's `effective_at`.
        PolicyEffective = "policy_effective",
        /// A hold release (five minutes later).
        HoldRelease = "hold_release",
    }
}

/// One retention policy row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RetentionPolicy {
    /// The row id.
    pub id: Id,
    /// Its level.
    pub scope: RetentionScope,
    /// Days messages live.
    pub messages_days: Option<u32>,
    /// Days files live.
    pub files_days: Option<u32>,
    /// Install only: days deleted content is kept.
    pub deleted_purge_days: Option<u32>,
    /// Lifecycle state.
    pub state: PolicyState,
    /// When it takes (or took) effect.
    pub effective_at: Timestamp,
    /// The preview a shorter value was set from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_id: Option<Id>,
    /// The member who set it (absent for the service and the seed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<Id>,
    /// Who set it.
    pub created_via: CreatedVia,
    /// When it was set.
    pub created_at: Timestamp,
    /// When it was superseded or cancelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Timestamp>,
}

/// `GET /api/v1/compliance/retention/effective?channel_id=`: what applies to one channel now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EffectiveRetention {
    /// The channel.
    pub channel_id: Id,
    /// Effective days for messages (`null` = forever).
    pub messages_days: Option<u32>,
    /// Effective days for files (`null` = forever).
    pub files_days: Option<u32>,
    /// The install's deleted-content window (`null` = off).
    pub deleted_purge_days: Option<u32>,
    /// The level that sets `messages_days` (the install when forever).
    pub messages_from: RetentionScope,
    /// The level that sets `files_days` (the install when forever).
    pub files_from: RetentionScope,
    /// A child value is above what its parents allow (the parent's shorter value wins).
    pub clamped: bool,
}

/// Query of `GET /api/v1/compliance/retention/effective`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EffectiveRetentionQuery {
    /// The channel.
    pub channel_id: Id,
}

impl Validate for EffectiveRetentionQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Query of `GET /api/v1/compliance/retention`: active and pending rows (all rows with
/// `history`), newest first, optionally for one level.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetentionListQuery {
    /// `install`, `space` or `channel`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_kind: Option<String>,
    /// The space or channel id (with `scope_kind`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<Id>,
    /// Include superseded and cancelled rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<bool>,
    /// Rows older than this id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Id>,
    /// Page size, 1–100 (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
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

impl Validate for RetentionListQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        match self.scope_kind.as_deref() {
            None => {
                if self.scope_id.is_some() {
                    return Err(ValidationError::invalid(
                        "scope_id",
                        "scope_id needs scope_kind",
                    ));
                }
            }
            Some("install") => {
                if self.scope_id.is_some() {
                    return Err(ValidationError::invalid(
                        "scope_id",
                        "the install scope has no id",
                    ));
                }
            }
            Some("space" | "channel") => {
                if self.scope_id.is_none() {
                    return Err(ValidationError::invalid(
                        "scope_id",
                        "space and channel scopes need scope_id",
                    ));
                }
            }
            Some(_) => {
                return Err(ValidationError::invalid(
                    "scope_kind",
                    "scope_kind is install, space or channel",
                ));
            }
        }
        check_limit(self.limit)
    }
}

/// Body of `POST /api/v1/compliance/retention/preview`: a proposal for one level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewRetentionRequest {
    /// The level.
    pub scope: RetentionScope,
    /// Proposed days for messages (`null` = forever / inherit).
    #[serde(default)]
    pub messages_days: Option<u32>,
    /// Proposed days for files.
    #[serde(default)]
    pub files_days: Option<u32>,
    /// Install only: proposed deleted-content window.
    #[serde(default)]
    pub deleted_purge_days: Option<u32>,
    /// When it would take effect (default now + 7 days; 24 h to 365 days ahead).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_at: Option<Timestamp>,
}

impl PreviewRetentionRequest {
    /// The proposed values.
    pub fn values(&self) -> RetentionValues {
        RetentionValues {
            messages_days: self.messages_days,
            files_days: self.files_days,
            deleted_purge_days: self.deleted_purge_days,
        }
    }
}

impl Validate for PreviewRetentionRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.values().check_bounds()
    }
}

/// Body of `POST /api/v1/compliance/retention/set`: exactly one form, `{preview_id}` (a
/// shorter value, from a finished preview) or `{scope, …values}` (not shorter: active at once).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetRetentionRequest {
    /// A finished preview of the proposal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_id: Option<Id>,
    /// The level (without `preview_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<RetentionScope>,
    /// Days for messages (without `preview_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages_days: Option<u32>,
    /// Days for files (without `preview_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_days: Option<u32>,
    /// Install only (without `preview_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_purge_days: Option<u32>,
}

impl SetRetentionRequest {
    /// The values of the direct form.
    pub fn values(&self) -> RetentionValues {
        RetentionValues {
            messages_days: self.messages_days,
            files_days: self.files_days,
            deleted_purge_days: self.deleted_purge_days,
        }
    }
}

impl Validate for SetRetentionRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        match (self.preview_id, self.scope) {
            (Some(_), None) => {
                if self.values() != RetentionValues::default() {
                    return Err(ValidationError::invalid(
                        "preview_id",
                        "a set from a preview takes no values",
                    ));
                }
                Ok(())
            }
            (None, Some(_)) => self.values().check_bounds(),
            (Some(_), Some(_)) => Err(ValidationError::invalid(
                "scope",
                "give preview_id or scope, not both",
            )),
            (None, None) => Err(ValidationError::invalid(
                "scope",
                "give preview_id or scope with values",
            )),
        }
    }
}

/// Counts a run or preview would destroy (or did). Ids and counts only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RetentionTotals {
    /// Live messages hidden (expired).
    pub messages_expired: u64,
    /// Messages whose content was destroyed (purged).
    pub messages_purged: u64,
    /// Messages past a cutoff kept by a legal hold.
    pub messages_held: u64,
    /// Edited-away revisions purged.
    pub revisions_purged: u64,
    /// Files hidden by file retention.
    pub files_removed: u64,
    /// Files purged.
    pub files_purged: u64,
    /// Files past a cutoff kept by a legal hold.
    pub files_held: u64,
    /// Plaintext bytes of the purged files.
    pub file_bytes_purged: u64,
    /// Drafts deleted.
    pub drafts_deleted: u64,
    /// Unsent scheduled messages deleted.
    pub scheduled_deleted: u64,
}

/// Per-channel counts of a preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelCounts {
    /// The channel.
    pub channel_id: Id,
    /// Live messages that would be hidden.
    pub messages_expired: u64,
    /// Messages that would be purged.
    pub messages_purged: u64,
    /// Messages a hold keeps.
    pub messages_held: u64,
    /// Files that would be hidden.
    pub files_removed: u64,
    /// Files that would be purged.
    pub files_purged: u64,
    /// Files a hold keeps.
    pub files_held: u64,
}

/// A retention preview (dry run): the real planner in counting mode at `as_of`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RetentionPreview {
    /// The preview id (`set {preview_id}` uses it).
    pub id: Id,
    /// Its `retention_preview` job.
    pub job_id: Id,
    /// Running, done or failed.
    pub state: RunState,
    /// The level proposed.
    pub scope: RetentionScope,
    /// The values proposed.
    pub proposal: RetentionValues,
    /// When the proposal would take effect (the counts are for this time).
    pub as_of: Timestamp,
    /// Whole-install totals, once done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totals: Option<RetentionTotals>,
    /// The top 100 channels by messages, then files.
    #[serde(default)]
    pub per_channel: Vec<ChannelCounts>,
    /// Children whose set value the proposal clamps.
    #[serde(default)]
    pub clamped_scopes: Vec<RetentionScope>,
    /// When it was requested.
    pub created_at: Timestamp,
    /// When it can no longer be used (24 h).
    pub expires_at: Timestamp,
}

/// One retention run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RetentionRun {
    /// The run id.
    pub id: Id,
    /// What started it.
    pub trigger: RunTrigger,
    /// The fixed time the run's cutoffs use.
    pub as_of: Timestamp,
    /// Running, done or failed.
    pub state: RunState,
    /// The phase it is in (or ended in).
    pub phase: String,
    /// What it destroyed.
    pub totals: RetentionTotals,
    /// Search documents deleted.
    pub search_docs_deleted: u64,
    /// Durable rows re-sealed under the current month.
    pub rows_resealed: u64,
    /// Durable blob links re-wrapped.
    pub links_rewrapped: u64,
    /// `key_shred` jobs enqueued.
    pub shreds_enqueued: u64,
    /// When it started.
    pub started_at: Timestamp,
    /// When it finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<Timestamp>,
}

/// Response of `POST …/retention/run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunRetentionResponse {
    /// The queued `retention_purge` job.
    pub job_id: Id,
}

/// Query of `GET …/retention/runs` (newest first).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunsQuery {
    /// Runs older than this id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Id>,
    /// Page size, 1–100 (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl Validate for RunsQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        check_limit(self.limit)
    }
}

/// A legal hold. Its name is plaintext metadata: never describe the case in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LegalHold {
    /// The hold id.
    pub id: Id,
    /// Its name (unique, case-insensitive).
    pub name: String,
    /// Member targets.
    pub members: Vec<Id>,
    /// Channel targets.
    pub channels: Vec<Id>,
    /// Space targets (channels created later included).
    pub spaces: Vec<Id>,
    /// Start of the range (inclusive; absent = open).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Timestamp>,
    /// End of the range (exclusive; absent = open).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Timestamp>,
    /// Who created it (absent for the service).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<Id>,
    /// When it was created.
    pub created_at: Timestamp,
    /// When it last changed.
    pub updated_at: Timestamp,
    /// When it was released.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_at: Option<Timestamp>,
    /// Who released it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_by: Option<Id>,
    /// Channels it covers now.
    pub coverage_channel_count: u32,
}

fn check_hold_name(field: &str, name: &str) -> Result<(), ValidationError> {
    let n = name.chars().count();
    if n == 0
        || n > HOLD_NAME_MAX_CHARS
        || name.trim() != name
        || name.chars().any(char::is_control)
    {
        return Err(ValidationError::invalid(
            field,
            format!(
                "{field} must be 1–{HOLD_NAME_MAX_CHARS} characters, trimmed, without control characters"
            ),
        ));
    }
    Ok(())
}

fn check_targets(field: &str, ids: &[Id]) -> Result<(), ValidationError> {
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

/// Body of `POST /api/v1/compliance/holds`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateHoldRequest {
    /// 1–100 characters, unique among all holds (case-insensitive).
    pub name: String,
    /// Member targets.
    #[serde(default)]
    pub members: Vec<Id>,
    /// Channel targets.
    #[serde(default)]
    pub channels: Vec<Id>,
    /// Space targets.
    #[serde(default)]
    pub spaces: Vec<Id>,
    /// Start of the range (inclusive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Timestamp>,
    /// End of the range (exclusive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Timestamp>,
}

impl Validate for CreateHoldRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_hold_name("name", &self.name)?;
        check_targets("members", &self.members)?;
        check_targets("channels", &self.channels)?;
        check_targets("spaces", &self.spaces)?;
        if self.members.is_empty() && self.channels.is_empty() && self.spaces.is_empty() {
            return Err(ValidationError::invalid(
                "members",
                "a hold needs at least one target",
            ));
        }
        if let (Some(f), Some(t)) = (self.from, self.to)
            && f >= t
        {
            return Err(ValidationError::invalid("to", "from must be before to"));
        }
        Ok(())
    }
}

/// Body of `POST /api/v1/compliance/holds/{id}/update`: additive only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateHoldRequest {
    /// A new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Members to add.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add_members: Vec<Id>,
    /// Channels to add.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add_channels: Vec<Id>,
    /// Spaces to add.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add_spaces: Vec<Id>,
    /// An earlier start, or `null` to open it.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub from: Patch<Timestamp>,
    /// A later end, or `null` to open it.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub to: Patch<Timestamp>,
}

impl Validate for UpdateHoldRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(n) = &self.name {
            check_hold_name("name", n)?;
        }
        check_targets("add_members", &self.add_members)?;
        check_targets("add_channels", &self.add_channels)?;
        check_targets("add_spaces", &self.add_spaces)
    }
}

/// Body of `POST /api/v1/compliance/holds/{id}/release`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReleaseHoldRequest {
    /// The hold's name, typed back exactly.
    pub confirm_name: String,
}

impl Validate for ReleaseHoldRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.confirm_name.is_empty() {
            return Err(ValidationError::invalid(
                "confirm_name",
                "confirm_name is the hold's name",
            ));
        }
        Ok(())
    }
}

unit_enum! {
    /// `GET …/holds?state=`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum HoldStateFilter {
        /// Unreleased holds.
        Active = "active",
        /// Released holds.
        Released = "released",
        /// Every hold.
        All = "all",
    }
}

/// Query of `GET /api/v1/compliance/holds` (newest first).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HoldsQuery {
    /// Which holds (default `all`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<HoldStateFilter>,
    /// Holds older than this id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Id>,
    /// Page size, 1–100 (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl Validate for HoldsQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(s) = self.state {
            check_known("state", s)?;
        }
        check_limit(self.limit)
    }
}

/// One channel as compliance sees it: metadata only, private channels and DMs included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ComplianceChannel {
    /// The channel.
    pub id: Id,
    /// Its space (absent for DMs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Id>,
    /// Its type.
    #[serde(rename = "type")]
    pub kind: ChannelType,
    /// Public or private.
    pub visibility: Visibility,
    /// The slug (absent for DMs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    /// The name (absent for DMs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// DM members (empty for space channels).
    #[serde(default)]
    pub dm_member_ids: Vec<Id>,
    /// Archived.
    pub archived: bool,
}

/// Query of `GET /api/v1/compliance/channels` (ascending ids after `after`… newest first with
/// `before`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComplianceChannelsQuery {
    /// Only this space's channels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Id>,
    /// Only this type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ChannelType>,
    /// Channels older than this id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Id>,
    /// Page size, 1–100 (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl ComplianceChannelsQuery {
    /// The cursor part.
    pub fn page(&self) -> PageQuery {
        PageQuery {
            before: self.before,
            after: None,
            around: None,
            limit: self.limit,
        }
    }
}

impl Validate for ComplianceChannelsQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(k) = self.kind {
            check_known("kind", k)?;
        }
        check_limit(self.limit)
    }
}

unit_enum! {
    /// Why content vanished.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum VanishReason {
        /// A retention policy's cutoff.
        Retention = "retention",
    }
}

/// An attachment that vanished from a live message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VanishedAttachment {
    /// The message.
    pub message_id: Id,
    /// The attachment.
    pub attachment_id: Id,
}

/// Body of `content_vanished`: one batch of retention expiry in one channel. Clients drop every
/// listed message (with its reactions and pin), keep a placeholder for `tombstone_ids` (roots
/// with live replies), and drop listed attachments. Not lossy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContentVanished {
    /// The channel.
    pub channel_id: Id,
    /// Why.
    pub reason: VanishReason,
    /// Messages that vanished.
    #[serde(default)]
    pub message_ids: Vec<Id>,
    /// Those of them that stay as a placeholder (roots with live replies).
    #[serde(default)]
    pub tombstone_ids: Vec<Id>,
    /// Attachments that vanished from live messages.
    #[serde(default)]
    pub attachments: Vec<VanishedAttachment>,
    /// The highest channel seq the batch appended.
    pub seq: u64,
}

/// `status.retention` on `GET /mgmt/v1/status` (S22 §18).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusRetention {
    /// When the last run finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<Timestamp>,
    /// How the last run ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_state: Option<RunState>,
    /// When the last run finished `done`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ok_at: Option<Timestamp>,
    /// A finite policy is set and no run finished `done` for 48 h.
    pub overdue: bool,
    /// Pending policies.
    pub pending_policies: u64,
    /// Unreleased holds.
    pub active_holds: u64,
    /// Purged messages whose search documents are not deleted yet.
    pub search_pending: u64,
    /// Scope-months waiting for a shred check.
    pub shred_candidates: u64,
}

/// `GET …/retention` response.
pub type RetentionPolicyPage = crate::http::Page<RetentionPolicy>;
/// `GET …/retention/runs` response.
pub type RetentionRunPage = crate::http::Page<RetentionRun>;
/// `GET …/holds` response.
pub type LegalHoldPage = crate::http::Page<LegalHold>;
/// `GET …/compliance/channels` response.
pub type ComplianceChannelPage = crate::http::Page<ComplianceChannel>;
