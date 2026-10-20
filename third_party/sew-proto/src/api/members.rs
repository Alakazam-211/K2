//! Members (C6, C7, C9).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::addr::Handle;
use crate::id::Id;
use crate::time::Timestamp;

unit_enum! {
    /// Whether a member is a person or an agent.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum MemberKind {
        /// A human.
        Person = "person",
        /// A K2 agent.
        Agent = "agent",
    }
}

unit_enum! {
    /// What kind of agent a member is.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum AgentKind {
        /// A team member in its own right.
        Coworker = "coworker",
        /// Pinned to one person; acts within that person's access (C9).
        Assistant = "assistant",
    }
}

unit_enum! {
    /// Install-wide role (C12). Separate from space roles.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum InstallRole {
        /// All install admin, plus transfer.
        Owner = "owner",
        /// Spaces, policies, members, guests. No content access to private channels or DMs.
        Admin = "admin",
        /// A regular member.
        Member = "member",
        /// A guest.
        Guest = "guest",
        /// A meeting link guest (Meet MC2a, C6 amendment CC1, CC3): made by redeeming a meeting
        /// link, bound to one meeting channel, capped by `LINK_GUEST_MASK`, never a K2
        /// principal. Every S19 guest rule applies, and only the link-guest allowlist passes.
        LinkGuest = "link_guest",
    }
}

impl InstallRole {
    /// A guest of either kind (S19 guest or link guest): every S19 guest rule applies.
    pub const fn is_guest_kind(self) -> bool {
        matches!(self, InstallRole::Guest | InstallRole::LinkGuest)
    }
}

/// A member of the install, person or agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Member {
    /// The member id.
    pub id: Id,
    /// Person or agent.
    pub kind: MemberKind,
    /// For agents, coworker or assistant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_kind: Option<AgentKind>,
    /// For assistants, the member id of their person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_of: Option<Id>,
    /// Install-wide role.
    pub install_role: InstallRole,
    /// The handle.
    pub username: Handle,
    /// The display name.
    pub display_name: String,
    /// Job title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Pronouns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pronouns: Option<String>,
    /// Signed avatar URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    /// Whether the member is deactivated.
    pub deactivated: bool,
    /// When the member was created.
    pub created_at: Timestamp,
    /// Email address; sent only to the member themself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Whether the member holds the compliance flag; sent only to the member themself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compliance: Option<bool>,
    /// 64 px signed avatar URL (S18).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_thumb_url: Option<String>,
    /// IANA timezone (S18).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// Status; absent when none or expired (S18).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<crate::api::people::MemberStatus>,
    /// Bumped by every change of this object; clients drop older events (S18).
    #[serde(default)]
    pub profile_rev: u64,
    /// Custom field values the viewer may see; only on `GET /members/{id}` and the self object
    /// (S18).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_fields: Option<Vec<crate::api::people::ProfileFieldValue>>,
    /// Fields the client must show read-only; self only (S18).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_fields: Option<Vec<String>>,
}

// ---------------------------------------------------------------- S10 additions (prd-sew-10 §8)

unit_enum! {
    /// An install role the install owner may give (S10 gap G10).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum SettableInstallRole {
        /// Install admin.
        Admin = "admin",
        /// Regular member.
        Member = "member",
    }
}

/// Body of `POST /api/v1/install/members/{id}/role` (install owner only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetInstallRoleRequest {
    /// The new role.
    pub install_role: SettableInstallRole,
}

impl crate::error::Validate for SetInstallRoleRequest {
    fn validate(&self) -> Result<(), crate::error::ValidationError> {
        crate::error::check_known("install_role", self.install_role)
    }
}
