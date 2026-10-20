//! Spaces over mgmt (SW11, SW13): K2 creates additional Spaces for `k2 chat` and its Apps,
//! binds a Space to an App, and adds or removes a Space's members (contracts rev 147: an App's
//! Space gets its members only from that App's user grants, through these calls).
//!
//! - `POST /mgmt/v1/spaces/create` ([`MgmtSpaceCreateRequest`] → [`MgmtSpaceResponse`]):
//!   the owner is `k2:owner` or a person, never an agent (S12: agents never own Spaces);
//!   `managers` (persons or coworker agents) join and hold the Space's manager role,
//!   [`crate::perms::SPACE_MANAGER`], which has no ADMINISTRATOR. Idempotent on `slug`: the same
//!   slug, owner and `app_id` again answers the existing Space (`created: false`) and adds any
//!   listed manager it lacks; another owner or App on that slug is `409 conflict`
//!   `slug_taken`.
//! - `POST /mgmt/v1/spaces/bind` ([`MgmtSpaceBindRequest`] → [`MgmtSpaceResponse`]): binds an
//!   unbound Space to an App, once. The same App again is a no-op; another is `409 conflict`
//!   `space_bound`.
//! - `POST /mgmt/v1/spaces/members/add` ([`MgmtSpaceMembersAddRequest`] →
//!   [`MgmtSpaceMembersAddResponse`]) and `…/members/remove`
//!   ([`MgmtSpaceMemberRemoveRequest`] → [`MgmtSpaceMemberRemoveResponse`]) for any Space, not
//!   only K2's default one. `app_id` must name the Space's App (absent for an unbound Space),
//!   else `409 conflict` `app_mismatch`. An add never fails for one member: a principal that
//!   cannot join comes back in `not_added` with a reason, and a member the Space bans in
//!   `skipped` (member ids, the HC2 shape of `members/add`; always empty until space bans
//!   land).
//!
//! On a bound Space the client API never adds members: `members/add`, the open join, a channel
//! add that would bring in a new Space member, and switching it to `open` are `422
//! invalid_state` `app_bound_space`. Leaving and moderator removal still work.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::addr::Slug;
use crate::error::{Validate, ValidationError, check_chars};
use crate::external_id::ExternalId;
use crate::id::Id;
use crate::limits::NAME_MAX_CHARS;

/// Longest App id, bytes.
pub const APP_ID_MAX_BYTES: usize = 64;
/// Most managers in one create.
pub const SPACE_MANAGERS_MAX: usize = 10;
/// Most principals in one mgmt add.
pub const SPACE_MEMBERS_ADD_MAX: usize = 100;
/// The manager role's name.
pub const SPACE_MANAGER_ROLE_NAME: &str = "Managers";

/// Whether `s` is an App id: 1–64 bytes of `[a-z0-9][a-z0-9._-]*`.
pub fn is_app_id(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=APP_ID_MAX_BYTES).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'.' || *c == b'_' || *c == b'-'
        })
}

fn check_app_id(field: &str, v: &str) -> Result<(), ValidationError> {
    if is_app_id(v) {
        Ok(())
    } else {
        Err(ValidationError::invalid(
            field,
            "an App id is 1–64 bytes of [a-z0-9][a-z0-9._-]*",
        ))
    }
}

fn check_opt_app_id(v: &Option<String>) -> Result<(), ValidationError> {
    v.as_deref().map_or(Ok(()), |a| check_app_id("app_id", a))
}

/// Body of `POST /mgmt/v1/spaces/create` (SW11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MgmtSpaceCreateRequest {
    /// The slug; `dm` is reserved. The idempotency key of the create.
    pub slug: Slug,
    /// The name, 1–100 characters.
    pub name: String,
    /// The owner: `k2:owner` or a person (never an agent).
    pub owner: ExternalId,
    /// 0–10 distinct members who join and hold the manager role (persons or coworker agents,
    /// not the owner).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub managers: Vec<ExternalId>,
    /// The App the Space belongs to (SW13); absent for a Space of K2's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// Meet MC1 (CC4): a hidden Space, left out of `GET /spaces`, browse, the directory and
    /// (without the `hidden_spaces` capability) READY. Set at create only. Default `false`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(v: &bool) -> bool {
    !*v
}

impl Validate for MgmtSpaceCreateRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        crate::api::spaces::check_space_slug("slug", &self.slug)?;
        check_chars("name", &self.name, 1, NAME_MAX_CHARS)?;
        if self.owner.is_agent() {
            return Err(ValidationError::invalid(
                "owner",
                "agents never own spaces; the owner is k2:owner or a person",
            ));
        }
        if self.managers.len() > SPACE_MANAGERS_MAX {
            return Err(ValidationError::invalid(
                "managers",
                format!("at most {SPACE_MANAGERS_MAX} managers"),
            ));
        }
        for (i, m) in self.managers.iter().enumerate() {
            if *m == self.owner {
                return Err(ValidationError::invalid(
                    format!("managers.{i}"),
                    "the owner is not listed as a manager",
                ));
            }
            if self.managers[..i].contains(m) {
                return Err(ValidationError::invalid(
                    format!("managers.{i}"),
                    "listed twice",
                ));
            }
        }
        check_opt_app_id(&self.app_id)
    }
}

/// A Space over mgmt: the answer of create and bind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MgmtSpaceResponse {
    /// The Space.
    pub space_id: Id,
    /// Its slug.
    pub slug: Slug,
    /// Its name.
    pub name: String,
    /// Its owner's member id.
    pub owner_member_id: Id,
    /// The App it belongs to; absent when not bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// Whether this call created it.
    pub created: bool,
    /// The manager role, once a manager was listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manager_role_id: Option<Id>,
    /// The listed managers' member ids, in request order (each a member holding the role).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub manager_member_ids: Vec<Id>,
    /// Meet MC1: the Space is hidden. Absent when `false`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
}

/// Body of `POST /mgmt/v1/spaces/bind` (SW13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MgmtSpaceBindRequest {
    /// The Space.
    pub space_id: Id,
    /// The App.
    pub app_id: String,
}

impl Validate for MgmtSpaceBindRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_app_id("app_id", &self.app_id)
    }
}

/// Body of `POST /mgmt/v1/spaces/members/add` (SW13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MgmtSpaceMembersAddRequest {
    /// The Space.
    pub space_id: Id,
    /// The Space's App; absent for an unbound Space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// 1–100 distinct principals.
    pub external_ids: Vec<ExternalId>,
}

impl Validate for MgmtSpaceMembersAddRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_opt_app_id(&self.app_id)?;
        if self.external_ids.is_empty() || self.external_ids.len() > SPACE_MEMBERS_ADD_MAX {
            return Err(ValidationError::invalid(
                "external_ids",
                format!("1–{SPACE_MEMBERS_ADD_MAX} principals"),
            ));
        }
        for (i, e) in self.external_ids.iter().enumerate() {
            if self.external_ids[..i].contains(e) {
                return Err(ValidationError::invalid(
                    format!("external_ids.{i}"),
                    "listed twice",
                ));
            }
        }
        Ok(())
    }
}

/// A principal and its member id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MgmtSpaceMemberRef {
    /// The principal.
    pub external_id: ExternalId,
    /// Its member id.
    pub member_id: Id,
}

unit_enum! {
    /// Why a principal was not added (SW13; bans are not a reason here: a banned member is in
    /// `skipped`, HC2).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum MgmtSpaceNotAddedReason {
        /// No member has this external id (provision first).
        NotProvisioned = "not_provisioned",
        /// The member is deactivated.
        Deactivated = "deactivated",
        /// An assistant never joins a Space (it reads through its person).
        Assistant = "assistant",
        /// A guest enters a Space only through a channel add (S19).
        Guest = "guest",
    }
}

/// A principal an add did not add, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MgmtSpaceNotAdded {
    /// The principal.
    pub external_id: ExternalId,
    /// Why.
    pub reason: MgmtSpaceNotAddedReason,
}

/// Response of `POST /mgmt/v1/spaces/members/add`, each list in request order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MgmtSpaceMembersAddResponse {
    /// Added by this call.
    pub added: Vec<MgmtSpaceMemberRef>,
    /// Members already.
    pub already: Vec<MgmtSpaceMemberRef>,
    /// Members the Space bans, not added (HC2's `members/add` shape: member ids, no reason;
    /// K2 stops re-adding them). Always present; empty until space bans land.
    #[serde(default)]
    pub skipped: Vec<Id>,
    /// Principals that cannot join, with the reason.
    pub not_added: Vec<MgmtSpaceNotAdded>,
}

/// Body of `POST /mgmt/v1/spaces/members/remove` (SW13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MgmtSpaceMemberRemoveRequest {
    /// The Space.
    pub space_id: Id,
    /// The Space's App; absent for an unbound Space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// The principal.
    pub external_id: ExternalId,
}

impl Validate for MgmtSpaceMemberRemoveRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_opt_app_id(&self.app_id)
    }
}

/// Response of `POST /mgmt/v1/spaces/members/remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MgmtSpaceMemberRemoveResponse {
    /// Whether this call removed a member (`false`: not provisioned or not a member).
    pub removed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_id_grammar() {
        for ok in ["hearth", "a", "app.v2", "k2_meet-1", &"a".repeat(64)] {
            assert!(is_app_id(ok), "{ok}");
        }
        for bad in ["", "Hearth", "-a", ".a", "a b", "a/b", &"a".repeat(65)] {
            assert!(!is_app_id(bad), "{bad}");
        }
    }
}
