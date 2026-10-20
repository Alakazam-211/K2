//! Channel and category overwrite bodies (S12 §7).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::spaces::{Overwrite, OverwriteTarget};
use crate::error::{Validate, ValidationError, check_known};
use crate::id::Id;
use crate::perms::Perms;

/// Body of `POST /api/v1/channels/{id}/overwrites/set` and the category route. `allow = deny =
/// 0` deletes the entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetOverwriteRequest {
    /// Role or member.
    pub target_kind: OverwriteTarget,
    /// The role (the space id = `@everyone`) or member.
    pub target_id: Id,
    /// Bits allowed (within OVERWRITABLE).
    pub allow: Perms,
    /// Bits denied (within OVERWRITABLE, none shared with `allow`).
    pub deny: Perms,
}

impl Validate for SetOverwriteRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_known("target_kind", self.target_kind)?;
        self.allow.validate_overwrite("allow")?;
        self.deny.validate_overwrite("deny")?;
        if self.allow.0 & self.deny.0 != 0 {
            return Err(ValidationError::invalid(
                "deny",
                "a bit cannot be both allowed and denied",
            ));
        }
        Ok(())
    }
}

/// Body of `POST /api/v1/channels/{id}/overwrites/delete` and the category route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OverwriteRef {
    /// Role or member.
    pub target_kind: OverwriteTarget,
    /// The role or member.
    pub target_id: Id,
}

impl Validate for OverwriteRef {
    fn validate(&self) -> Result<(), ValidationError> {
        check_known("target_kind", self.target_kind)
    }
}

/// A channel's overwrites as they apply (`GET /api/v1/channels/{id}/overwrites` and the edit
/// routes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChannelOverwrites {
    /// The channel.
    pub channel_id: Id,
    /// Its space.
    pub space_id: Id,
    /// Its category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category_id: Option<Id>,
    /// Whether the channel follows its category.
    pub synced: bool,
    /// The effective list: the category's while synced, else the channel's own.
    pub overwrites: Vec<Overwrite>,
}
