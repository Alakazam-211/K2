//! Timeout and slow-mode bodies (S12 §7).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Validate, ValidationError};
use crate::limits::{SLOW_MODE_MAX_S, TIMEOUT_MAX_MS, TIMEOUT_MIN_MS};

/// Body of `POST /api/v1/spaces/{id}/members/{member_id}/timeout`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimeoutRequest {
    /// 60,000 – 2,419,200,000 ms (1 min – 28 days).
    pub duration_ms: u64,
}

impl Validate for TimeoutRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if !(TIMEOUT_MIN_MS..=TIMEOUT_MAX_MS).contains(&self.duration_ms) {
            return Err(ValidationError::invalid(
                "duration_ms",
                format!("duration_ms must be {TIMEOUT_MIN_MS}–{TIMEOUT_MAX_MS}"),
            ));
        }
        Ok(())
    }
}

/// Body of `POST /api/v1/channels/{id}/slow_mode/set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SlowModeRequest {
    /// Seconds between posts per member, 0 – 21,600; 0 turns it off.
    pub slow_mode_s: u32,
}

impl Validate for SlowModeRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.slow_mode_s > SLOW_MODE_MAX_S {
            return Err(ValidationError::invalid(
                "slow_mode_s",
                format!("slow_mode_s must be 0–{SLOW_MODE_MAX_S}"),
            ));
        }
        Ok(())
    }
}
