//! Role CRUD, reorder and assignment bodies (S12 §7).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::spaces::Role;
use crate::error::{Validate, ValidationError, check_unique_ids};
use crate::http::{Page, Patch};
use crate::id::Id;
use crate::limits::{ROLE_IDS_PER_REQUEST, ROLE_NAME_MAX, ROLES_PER_SPACE};
use crate::perms::Perms;

/// Largest role color (`0xRRGGBB`).
pub const ROLE_COLOR_MAX: u32 = 0x00FF_FFFF;

fn check_role_name(field: &str, name: &str) -> Result<(), ValidationError> {
    let n = name.chars().count();
    if n == 0 || n > ROLE_NAME_MAX || name.chars().any(char::is_control) {
        return Err(ValidationError::invalid(
            field,
            format!("{field} must be 1–{ROLE_NAME_MAX} characters without control characters"),
        ));
    }
    Ok(())
}

fn check_color(color: Option<&u32>) -> Result<(), ValidationError> {
    if let Some(c) = color
        && *c > ROLE_COLOR_MAX
    {
        return Err(ValidationError::invalid(
            "color",
            "color must be 0xRRGGBB (at most 16777215)",
        ));
    }
    Ok(())
}

/// Body of `POST /api/v1/spaces/{id}/roles`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateRoleRequest {
    /// The name, 1–100 characters, no control characters.
    pub name: String,
    /// Color as `0xRRGGBB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    /// Bits the role grants; default none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perms: Option<Perms>,
    /// Whether anyone may `@mention` it; default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mentionable: Option<bool>,
    /// Whether holders are listed separately; default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hoist: Option<bool>,
    /// Ladder position, 1 ≤ p ≤ the current role count (top + 1); default 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
}

impl Validate for CreateRoleRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_role_name("name", &self.name)?;
        check_color(self.color.as_ref())?;
        if let Some(p) = self.perms {
            p.validate_request("perms")?;
        }
        if self.position == Some(0) {
            return Err(ValidationError::invalid(
                "position",
                "position 0 belongs to @everyone",
            ));
        }
        Ok(())
    }
}

/// Body of `POST /api/v1/roles/{id}/edit`. At least one field.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateRoleRequest {
    /// A new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A new color; `null` clears it.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub color: Patch<u32>,
    /// New bits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perms: Option<Perms>,
    /// Whether anyone may `@mention` it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mentionable: Option<bool>,
    /// Whether holders are listed separately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hoist: Option<bool>,
}

impl Validate for UpdateRoleRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.name.is_none()
            && self.color.is_keep()
            && self.perms.is_none()
            && self.mentionable.is_none()
            && self.hoist.is_none()
        {
            return Err(ValidationError::invalid("", "nothing to change"));
        }
        if let Some(n) = &self.name {
            check_role_name("name", n)?;
        }
        check_color(self.color.as_set())?;
        if let Some(p) = self.perms {
            p.validate_request("perms")?;
        }
        Ok(())
    }
}

/// Body of `POST /api/v1/spaces/{id}/roles/reorder`: every role of the space except
/// `@everyone`, bottom to top (index 0 → position 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReorderRolesRequest {
    /// Distinct role ids, bottom to top.
    pub role_ids: Vec<Id>,
}

impl Validate for ReorderRolesRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_unique_ids("role_ids", &self.role_ids, 0, ROLES_PER_SPACE as usize)
    }
}

/// Body of `POST /api/v1/spaces/{id}/members/{member_id}/roles/add` and `…/remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemberRolesRequest {
    /// 1–25 distinct role ids.
    pub role_ids: Vec<Id>,
}

impl Validate for MemberRolesRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_unique_ids("role_ids", &self.role_ids, 1, ROLE_IDS_PER_REQUEST)
    }
}

/// A space's roles, ascending by position, `@everyone` first (`has_more` is always false).
pub type RolePage = Page<Role>;
