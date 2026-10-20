//! Standalone local users over the mgmt socket (contracts revs 9 and 17; S24 §7.2).
//!
//! `POST /mgmt/v1/local/users`, `/local/setup-link`, `/local/users/reactivate` and
//! `/local/users/role`, and S19's `/local/users/guest`. A local user's external id is `local:<username>`, so its handle is fixed
//! and stays reserved while deactivated. On a K2 install every route answers
//! `403 provider_is_k2` before it reads the body.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::addr::Handle;
use crate::api::members::InstallRole;
use crate::error::{Validate, ValidationError, check_chars, check_known};
use crate::id::Id;
use crate::limits::{DISPLAY_NAME_MAX_CHARS, EMAIL_MAX_BYTES};
use crate::time::Timestamp;

unit_enum! {
    /// The install role a new local user gets (never `owner`; `sew init` makes the owner).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum LocalRole {
        /// A regular member (the default).
        Member = "member",
        /// An install admin.
        Admin = "admin",
        /// A guest.
        Guest = "guest",
    }
}

impl LocalRole {
    /// The install role.
    pub fn install_role(self) -> InstallRole {
        match self {
            LocalRole::Admin => InstallRole::Admin,
            LocalRole::Guest => InstallRole::Guest,
            _ => InstallRole::Member,
        }
    }
}

unit_enum! {
    /// An install role that `local/users/role` may set (S10's settable roles).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum LocalSettableRole {
        /// An install admin.
        Admin = "admin",
        /// A regular member.
        Member = "member",
    }
}

impl LocalSettableRole {
    /// The install role.
    pub fn install_role(self) -> InstallRole {
        match self {
            LocalSettableRole::Admin => InstallRole::Admin,
            _ => InstallRole::Member,
        }
    }
}

fn default_member() -> LocalRole {
    LocalRole::Member
}

fn default_true() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

fn is_member(r: &LocalRole) -> bool {
    *r == LocalRole::Member
}

fn check_display_name(field: &str, name: &str) -> Result<(), ValidationError> {
    check_chars(field, name, 1, DISPLAY_NAME_MAX_CHARS)
}

/// Body of `POST /mgmt/v1/local/users`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LocalUserCreateRequest {
    /// The handle; the external id becomes `local:<username>`.
    pub username: Handle,
    /// Display name, 1–100 characters.
    pub display_name: String,
    /// Install role (default `member`).
    #[serde(default = "default_member", skip_serializing_if = "is_member")]
    pub install_role: LocalRole,
    /// Email address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Also issue a one-time password setup link (default `true`).
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub setup_link: bool,
}

impl Validate for LocalUserCreateRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_known("install_role", self.install_role)?;
        check_display_name("display_name", &self.display_name)?;
        if let Some(e) = &self.email
            && (e.is_empty() || e.len() > EMAIL_MAX_BYTES || !e.contains('@'))
        {
            return Err(ValidationError::invalid(
                "email",
                format!("email must be an address of at most {EMAIL_MAX_BYTES} bytes"),
            ));
        }
        Ok(())
    }
}

/// Body of `POST /mgmt/v1/local/setup-link`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetupLinkRequest {
    /// The local user.
    pub username: Handle,
    /// Create this user as the first install owner (`sew init`); refused once an owner exists.
    #[serde(default)]
    pub owner: bool,
    /// The owner's display name (only with `owner: true`; default the username).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

impl Validate for SetupLinkRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        match (&self.display_name, self.owner) {
            (Some(_), false) => Err(ValidationError::invalid(
                "display_name",
                "display_name is only for owner: true",
            )),
            (Some(n), true) => check_display_name("display_name", n),
            (None, _) => Ok(()),
        }
    }
}

/// Body of `POST /mgmt/v1/local/users/reactivate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LocalUserRef {
    /// The local user.
    pub username: Handle,
}

impl Validate for LocalUserRef {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Body of `POST /mgmt/v1/local/users/role`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LocalRoleRequest {
    /// The local user.
    pub username: Handle,
    /// The new install role.
    pub install_role: LocalSettableRole,
}

impl Validate for LocalRoleRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_known("install_role", self.install_role)
    }
}

/// Body of `POST /mgmt/v1/local/users/guest` (S19 §19): make a local user a guest (`true`) or a
/// member (`false`). Answers [`LocalUserResponse`]; `changed: false` when nothing changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LocalGuestRequest {
    /// The local user.
    pub username: Handle,
    /// Guest or member.
    pub guest: bool,
}

impl Validate for LocalGuestRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// A one-time password setup link. `Debug` redacts the URL (it holds the token).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SetupLink {
    /// `<setup_url>#token=sewpw_…`.
    pub url: String,
    /// When the link stops working (24 h).
    pub expires_at: Timestamp,
}

impl fmt::Debug for SetupLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetupLink")
            .field("url", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Response of every `/mgmt/v1/local/*` route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LocalUserResponse {
    /// The member.
    pub member_id: Id,
    /// The handle.
    pub username: Handle,
    /// The install role.
    pub install_role: InstallRole,
    /// Whether the member is deactivated.
    pub deactivated: bool,
    /// Whether this call created the member.
    pub created: bool,
    /// Whether this call reactivated the member.
    pub reactivated: bool,
    /// Whether this call changed anything.
    pub changed: bool,
    /// A fresh setup link, when one was issued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_link: Option<SetupLink>,
}
