//! Guests (prd-sew-19 §18, §19; contracts rev 23): conversion between member and guest, and the
//! install admins' guest list.
//!
//! - `POST /api/v1/install/members/{id}/guest` ([`SetGuestRequest`] → [`GuestConvertResponse`]):
//!   standalone only (`403 provider_is_k2` on K2, where the grant scope decides).
//! - `GET /api/v1/install/guests` ([`GuestListQuery`] → `Page<GuestSummary>`): install owner and
//!   admins only. Channel names are shown for private channels too (C12: admins see private
//!   names, never content).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::channels::Visibility;
use crate::api::members::Member;
use crate::error::{Validate, ValidationError};
use crate::id::Id;
use crate::limits::PAGE_LIMIT_MAX;

/// Default page size of [`GuestListQuery`].
pub const GUEST_LIST_LIMIT_DEFAULT: u32 = 50;

/// Body of `POST /api/v1/install/members/{id}/guest`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetGuestRequest {
    /// `true`: member → guest; `false`: guest → member.
    pub guest: bool,
}

impl Validate for SetGuestRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Response of `POST /api/v1/install/members/{id}/guest`: the member after the change and what
/// the conversion took away (S19 §7.4). `changed: false` when the member already had that
/// status (nothing written).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GuestConvertResponse {
    /// The member (self view fields left out).
    pub member: Member,
    /// Whether anything changed.
    pub changed: bool,
    /// Space roles removed because they hold bits outside the guest mask.
    pub roles_removed: Vec<Id>,
    /// Member overwrites whose allow bits were trimmed to the guest mask (or deleted).
    pub overwrites_trimmed: u32,
    /// Spaces whose ownership moved to the install owner.
    pub spaces_transferred: Vec<Id>,
    /// Spaces left because no channel membership remained there.
    pub spaces_left: Vec<Id>,
    /// User groups left (guests are never group members).
    pub groups_left: u32,
}

/// One channel a guest is in, for the admins' guest list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GuestChannelRef {
    /// The channel.
    pub channel_id: Id,
    /// Its space.
    pub space_id: Id,
    /// The channel name (private channels included).
    pub name: String,
    /// Public or private.
    pub visibility: Visibility,
}

/// One guest in `GET /api/v1/install/guests`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GuestSummary {
    /// The guest.
    pub member: Member,
    /// Spaces the guest is in, ascending.
    pub spaces: Vec<Id>,
    /// Space channels the guest is in, ascending by channel id (DMs are not listed).
    pub channels: Vec<GuestChannelRef>,
}

/// Query of `GET /api/v1/install/guests`, ordered by member id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GuestListQuery {
    /// Guests after this member id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Id>,
    /// Page size, 1–100 (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Include deactivated guests (default `false`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_deactivated: Option<bool>,
}

impl GuestListQuery {
    /// The page size.
    pub fn limit(&self) -> u32 {
        self.limit.unwrap_or(GUEST_LIST_LIMIT_DEFAULT)
    }
}

impl Validate for GuestListQuery {
    fn validate(&self) -> Result<(), ValidationError> {
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

/// A page of guests.
pub type GuestSummaryPage = crate::http::Page<GuestSummary>;
