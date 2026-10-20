//! Meet MC1 over mgmt (prd-meet-01 §6.2, CC7): `POST /mgmt/v1/meet/init` creates the install's
//! hidden Meetings Space (slug `meetings`) and its retention, once.
//!
//! - The owner is `k2:owner` on a K2 install and the install owner on a standalone one (SW13's
//!   rule: never an agent). `@everyone` holds the channel bits 0–3, 6, 7, 11, 13 and 16–19 and
//!   no space bit, so nobody creates channels there through the generic route (only
//!   `POST /api/v1/meetings` does).
//! - `app_id` binds the Space to the Meet App (SW13): its members then come only from mgmt
//!   `spaces/members/add` for that App. Absent on a standalone install: any active non-guest
//!   member joins it on their first meeting.
//! - Retention: one S22 policy on the Space, chat and transcripts `retention_days` (default
//!   365). An install whose own retention is shorter keeps its own (`retention_days` is then
//!   absent in the answer). Meeting audio is kept 30 days (each meeting channel's
//!   `recording_audio_days`).
//! - Idempotent: a second call answers the same Space with `created: false` (it binds an
//!   unbound Space to `app_id` once, as `spaces/bind` would; another App is `409 conflict`
//!   `space_bound`). A non-hidden Space holding the slug `meetings` is `409 conflict`
//!   `slug_taken`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Validate, ValidationError};
use crate::id::Id;

/// Longest `retention_days` (100 years).
pub const MEET_RETENTION_DAYS_MAX: u32 = 36_500;

/// Body of `POST /mgmt/v1/meet/init`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeetInitRequest {
    /// The Meet App the Space belongs to (SW13); absent on a standalone install.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// Days chat and transcripts are kept (1–36,500; default 365). Used when the Space is
    /// made; a later call never changes it (S22's routes do).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_days: Option<u32>,
}

impl Validate for MeetInitRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(a) = &self.app_id
            && !super::is_app_id(a)
        {
            return Err(ValidationError::invalid(
                "app_id",
                "an App id is 1–64 bytes of [a-z0-9][a-z0-9._-]*",
            ));
        }
        if let Some(d) = self.retention_days
            && !(1..=MEET_RETENTION_DAYS_MAX).contains(&d)
        {
            return Err(ValidationError::invalid(
                "retention_days",
                format!("retention_days must be 1–{MEET_RETENTION_DAYS_MAX}"),
            ));
        }
        Ok(())
    }
}

/// Answer of `POST /mgmt/v1/meet/init`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeetInitResponse {
    /// The Meetings Space.
    pub space_id: Id,
    /// Whether this call created it.
    pub created: bool,
    /// The App it belongs to; absent when not bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// The Space's chat and transcript retention, in days; absent when the install's shorter
    /// retention applies instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_days: Option<u32>,
}
