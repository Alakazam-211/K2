//! Ops bodies over the mgmt socket that S24 adds (contracts rev 17; S24 §7.2): the audit chain
//! check (`POST /mgmt/v1/audit/verify`, gap G5) and the logical export request (gap G9; S23 owns
//! the route and may add fields additively).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Validate, ValidationError};

unit_enum! {
    /// Where an audit chain check starts.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum AuditVerifyMode {
        /// From the last saved checkpoint (the daily schedule's mode).
        Incremental = "incremental",
        /// The whole chain from `chain_seq` 1.
        Full = "full",
    }
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// Body of `POST /mgmt/v1/audit/verify`. Returns `StartJobResponse`; the job's `result` is an
/// [`AuditVerifyReport`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuditVerifyRequest {
    /// Check the whole chain instead of the part after the last checkpoint.
    #[serde(default, skip_serializing_if = "is_false")]
    pub full: bool,
}

impl Validate for AuditVerifyRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// What an `audit_verify` job found: `JobStatus.result` of a succeeded `audit_verify` job. A
/// broken chain is a successful job whose report has `broken_at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuditVerifyReport {
    /// Incremental or full.
    pub mode: AuditVerifyMode,
    /// Events that passed.
    pub checked: u64,
    /// The last good `chain_seq`.
    pub head_seq: u64,
    /// Its hash, 64 lowercase hex chars.
    pub head_hash: String,
    /// The first failing `chain_seq`; `null` when the chain is intact.
    pub broken_at: Option<u64>,
    /// Why it failed; `null` when the chain is intact.
    pub reason: Option<String>,
}

/// Most recipients of one export.
pub const EXPORT_RECIPIENTS_MAX: usize = 16;

/// Body of `POST /mgmt/v1/export` (gap G9): a full logical export encrypted to age recipients.
/// Returns `StartJobResponse`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportRequest {
    /// age X25519 recipients (`age1…`), 1–16.
    pub recipients: Vec<String>,
}

impl Validate for ExportRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.recipients.is_empty() || self.recipients.len() > EXPORT_RECIPIENTS_MAX {
            return Err(ValidationError::invalid(
                "recipients",
                format!("recipients must hold 1–{EXPORT_RECIPIENTS_MAX} age recipients"),
            ));
        }
        if self
            .recipients
            .iter()
            .any(|r| !r.starts_with("age1") || r.len() > 128)
        {
            return Err(ValidationError::invalid(
                "recipients",
                "every recipient must be an age1… X25519 recipient",
            ));
        }
        Ok(())
    }
}
