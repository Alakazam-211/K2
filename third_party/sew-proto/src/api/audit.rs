//! The audit query API (prd-sew-23 §7): `GET /api/v1/audit`, `GET /api/v1/audit/chain`,
//! `POST /api/v1/audit/verify`, and the mgmt twin `GET /mgmt/v1/audit`.
//!
//! Who reads what follows each action's [`AuditVisibility`](crate::audit::AuditVisibility): compliance
//! holders and the mgmt service read everything, the install owner reads `install` and `space`
//! rows, a `VIEW_AUDIT_LOG` holder reads `space` rows of that space.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::audit::{AuditEvent, AuditResult};
use crate::error::{Validate, ValidationError, check_known};
use crate::id::Id;
use crate::limits::PAGE_LIMIT_MAX;
use crate::mgmt::{AuditVerifyMode, AuditVerifyReport};
use crate::time::Timestamp;

/// Most exact action names one query may list.
pub const AUDIT_ACTIONS_MAX: usize = 20;

/// Query of `GET /api/v1/audit` and `GET /mgmt/v1/audit`. Filters combine with AND; at most
/// one cursor. `from`/`to` map to id bounds (ids are time-sortable).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditQuery {
    /// Events of this space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<Id>,
    /// Events of this channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<Id>,
    /// Events by this member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<Id>,
    /// The target kind (with `target_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_kind: Option<String>,
    /// The target id (with `target_kind`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<Id>,
    /// Exact action names (repeatable in the query string, at most 20).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub action: Vec<String>,
    /// An action prefix such as `channel.`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_prefix: Option<String>,
    /// `ok`, `denied` or `error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<AuditResult>,
    /// Events at or after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Timestamp>,
    /// Events before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Timestamp>,
    /// Events older than this id (newest first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Id>,
    /// Events newer than this id (oldest first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Id>,
    /// Page size, 1–100 (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

fn check_name(field: &str, s: &str, prefix: bool) -> Result<(), ValidationError> {
    let ok = !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'.')
        && (prefix || s.contains('.'));
    if ok {
        Ok(())
    } else {
        Err(ValidationError::invalid(
            field,
            format!("{field} must be an action name like channel.archive"),
        ))
    }
}

impl Validate for AuditQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.target_kind.is_some() != self.target_id.is_some() {
            return Err(ValidationError::invalid(
                "target_kind",
                "give target_kind and target_id together",
            ));
        }
        if let Some(k) = &self.target_kind
            && (k.is_empty()
                || k.len() > 64
                || !k
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_'))
        {
            return Err(ValidationError::invalid(
                "target_kind",
                "target_kind is lower_snake",
            ));
        }
        if self.action.len() > AUDIT_ACTIONS_MAX {
            return Err(ValidationError::invalid(
                "action",
                format!("at most {AUDIT_ACTIONS_MAX} actions"),
            ));
        }
        for a in &self.action {
            check_name("action", a, false)?;
        }
        if let Some(p) = &self.action_prefix {
            check_name("action_prefix", p, true)?;
        }
        if let Some(r) = self.result {
            check_known("result", r)?;
        }
        if self.before.is_some() && self.after.is_some() {
            return Err(ValidationError::invalid(
                "before",
                "at most one of before, after",
            ));
        }
        if let (Some(f), Some(t)) = (self.from, self.to)
            && f >= t
        {
            return Err(ValidationError::invalid("to", "from must be before to"));
        }
        if let Some(l) = self.limit
            && !(1..=PAGE_LIMIT_MAX).contains(&l)
        {
            return Err(ValidationError::invalid(
                "limit",
                format!("limit must be 1–{PAGE_LIMIT_MAX}"),
            ));
        }
        Ok(())
    }
}

/// One chained audit row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AuditEntry {
    /// Its chain position.
    pub chain_seq: u64,
    /// Its hash, 64 hex.
    pub hash: String,
    /// The previous row's hash, 64 hex.
    pub prev_hash: String,
    /// The stored event.
    pub event: AuditEvent,
}

/// Response of `GET /api/v1/audit`, `GET /mgmt/v1/audit`.
pub type AuditEntryPage = crate::http::Page<AuditEntry>;

/// The newest finished `audit_verify` job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LastVerify {
    /// The job.
    pub job_id: Id,
    /// Incremental or full.
    pub mode: AuditVerifyMode,
    /// When it finished.
    pub finished_at: Timestamp,
    /// What it found.
    pub report: AuditVerifyReport,
}

/// Response of `GET /api/v1/audit/chain`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuditChainStatus {
    /// The highest `chain_seq`.
    pub head_seq: u64,
    /// Its hash, 64 hex (empty for an empty chain).
    pub head_hash: String,
    /// The last incrementally verified seq.
    pub verified_seq: u64,
    /// When the chain was last checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<Timestamp>,
    /// The first broken seq ever found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broken_at_seq: Option<u64>,
    /// Events waiting for the chainer.
    pub outbox_len: u64,
    /// The newest finished verify job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_verify: Option<LastVerify>,
}
