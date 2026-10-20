//! Spaces, categories, roles and overwrites (C12; S10, S12).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::addr::Slug;
use crate::error::{
    Validate, ValidationError, check_chars, check_known, check_opt_chars, check_unique_ids,
};
use crate::http::Page;
use crate::http::Patch;
use crate::id::Id;
use crate::limits::{
    NAME_MAX_CHARS, PAGE_LIMIT_MAX, SPACE_DEFAULT_CHANNELS_MAX, SPACE_DESCRIPTION_MAX_CHARS,
};
use crate::perms::Perms;
use crate::time::Timestamp;

pub(crate) fn check_space_slug(field: &str, slug: &Slug) -> Result<(), ValidationError> {
    if slug.is_reserved_for_space() {
        return Err(ValidationError::invalid(
            field,
            "the space slug \"dm\" is reserved",
        ));
    }
    Ok(())
}

/// A space: a group of channels with its own roles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Space {
    /// The space id.
    pub id: Id,
    /// The slug, unique in the install.
    pub slug: Slug,
    /// The name.
    pub name: String,
    /// Signed icon URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    /// Signed 64 px icon URL (S16).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_thumb_url: Option<String>,
    /// The space owner's member id.
    pub owner_id: Id,
    /// Number of members.
    pub member_count: u32,
    /// When the space was created.
    pub created_at: Timestamp,
    /// Who may join by themself (S10 gap G4). Old payloads without it read as `invite`.
    #[serde(default = "SpaceJoinPolicy::default_invite")]
    pub join_policy: SpaceJoinPolicy,
    /// The viewer's base permissions in the space; absent for non-members (S10 §8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_perms: Option<Perms>,
    /// Hearth HC6: the channels a joining member lands in (the open join and an add), in the
    /// order MANAGE_SPACE set them; at most 10, absent when none. Guests never see it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_channel_ids: Vec<Id>,
    /// Hearth HC4: what the space is for, 1–300 characters; absent when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Hearth HC4: the channel holding the space's rules (a public text or announcement
    /// channel of the space); absent when unset. Guests never see it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules_channel_id: Option<Id>,
    /// Hearth HC4: where a newcomer starts (a public text or announcement channel of the
    /// space); absent when unset. Guests never see it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub welcome_channel_id: Option<Id>,
    /// SW13: the App the space belongs to (`[a-z0-9][a-z0-9._-]{0,63}`); absent when the space
    /// is not bound. A bound space gets its members only through mgmt calls for that App.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// Meet MC1 (CC4): a hidden Space (the Meetings Space) is left out of `GET /spaces`,
    /// browse and the directory, and out of READY unless IDENTIFY declares `hidden_spaces`;
    /// clients read it by id. Absent when `false`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(v: &bool) -> bool {
    !*v
}

/// Body of the create-space route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateSpaceRequest {
    /// The slug; `dm` is reserved.
    pub slug: Slug,
    /// The name, 1–100 characters.
    pub name: String,
    /// Who may join by themself; default `invite` (S10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join_policy: Option<SpaceJoinPolicy>,
}

impl Validate for CreateSpaceRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_space_slug("slug", &self.slug)?;
        check_chars("name", &self.name, 1, NAME_MAX_CHARS)?;
        if let Some(p) = self.join_policy {
            check_known("join_policy", p)?;
        }
        Ok(())
    }
}

/// Body of the update-space route.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateSpaceRequest {
    /// A new slug.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<Slug>,
    /// A new name, 1–100 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A new join policy (S10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join_policy: Option<SpaceJoinPolicy>,
    /// New default channels (Hearth HC6): 0–10 distinct public, unarchived channels of the
    /// space; `[]` clears them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_channel_ids: Option<Vec<Id>>,
    /// Set or clear the description (Hearth HC4; ≤ 300 characters; a blank one clears).
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub description: Patch<String>,
    /// Set or clear the rules channel (Hearth HC4): a public, unarchived text or announcement
    /// channel of the space.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub rules_channel_id: Patch<Id>,
    /// Set or clear the welcome channel (Hearth HC4), with the rules channel's rules.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub welcome_channel_id: Patch<Id>,
}

impl Validate for UpdateSpaceRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(slug) = &self.slug {
            check_space_slug("slug", slug)?;
        }
        if let Some(p) = self.join_policy {
            check_known("join_policy", p)?;
        }
        if let Some(ids) = &self.default_channel_ids {
            check_unique_ids("default_channel_ids", ids, 0, SPACE_DEFAULT_CHANNELS_MAX)?;
        }
        check_opt_chars(
            "description",
            self.description.as_set().map(String::as_str),
            0,
            SPACE_DESCRIPTION_MAX_CHARS,
        )?;
        check_opt_chars("name", self.name.as_deref(), 1, NAME_MAX_CHARS)
    }
}

unit_enum! {
    /// What an overwrite targets.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum OverwriteTarget {
        /// A role (`@everyone`'s role id is the space id).
        Role = "role",
        /// A single member.
        Member = "member",
    }
}

/// A channel or category permission overwrite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Overwrite {
    /// Role or member.
    pub target_kind: OverwriteTarget,
    /// The role or member id.
    pub target_id: Id,
    /// Bits allowed.
    pub allow: Perms,
    /// Bits denied.
    pub deny: Perms,
}

impl Validate for Overwrite {
    fn validate(&self) -> Result<(), ValidationError> {
        check_known("target_kind", self.target_kind)?;
        self.allow.validate_overwrite("allow")?;
        self.deny.validate_overwrite("deny")
    }
}

/// A category grouping channels in a space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Category {
    /// The category id.
    pub id: Id,
    /// Its space.
    pub space_id: Id,
    /// The name.
    pub name: String,
    /// Sort position.
    pub position: u32,
    /// Overwrites that synced channels inherit.
    pub overwrites: Vec<Overwrite>,
}

/// Body of the create-category route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateCategoryRequest {
    /// The name, 1–100 characters.
    pub name: String,
    /// Sort position; default last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
}

impl Validate for CreateCategoryRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_chars("name", &self.name, 1, NAME_MAX_CHARS)
    }
}

/// Body of the update-category route.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateCategoryRequest {
    /// A new name, 1–100 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A new position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
}

impl Validate for UpdateCategoryRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_opt_chars("name", self.name.as_deref(), 1, NAME_MAX_CHARS)
    }
}

/// A space role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Role {
    /// The role id; `@everyone`'s equals the space id (gap G24).
    pub id: Id,
    /// Its space.
    pub space_id: Id,
    /// The name.
    pub name: String,
    /// Color as `0xRRGGBB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    /// Ladder position; `@everyone` is 0.
    pub position: u32,
    /// Bits the role grants.
    pub perms: Perms,
    /// Whether anyone may `@mention` it.
    pub mentionable: bool,
    /// Whether this is the `@everyone` role.
    pub is_everyone: bool,
    /// Whether members with this role are listed separately (S12). Old payloads read as false.
    #[serde(default)]
    pub hoist: bool,
}

// ---------------------------------------------------------------- S10 additions (prd-sew-10 §8)

unit_enum! {
    /// Who may join a space by themself (S10 gap G4).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum SpaceJoinPolicy {
        /// Any active non-guest member may join.
        Open = "open",
        /// Members join only when someone adds them (the default).
        Invite = "invite",
    }
}

impl SpaceJoinPolicy {
    /// `invite`, the default (also the serde default for old payloads).
    pub const fn default_invite() -> SpaceJoinPolicy {
        SpaceJoinPolicy::Invite
    }
}

unit_enum! {
    /// Which spaces `GET /api/v1/spaces` lists.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum SpaceListScope {
        /// Spaces the caller belongs to (the default).
        Mine = "mine",
        /// Open spaces the caller is not in (empty for guests).
        Joinable = "joinable",
        /// Every space (install owners and admins only).
        All = "all",
    }
}

/// Query of `GET /api/v1/spaces`: ordered by id ascending with an `after` cursor.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpaceListQuery {
    /// Which spaces; default `mine`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<SpaceListScope>,
    /// Spaces with a larger id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Id>,
    /// Page size, 1–100 (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl Validate for SpaceListQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(s) = self.scope {
            check_known("scope", s)?;
        }
        check_page_limit(self.limit)
    }
}

/// `limit` must be 1–100 when set.
pub(crate) fn check_page_limit(limit: Option<u32>) -> Result<(), ValidationError> {
    if let Some(l) = limit
        && !(1..=PAGE_LIMIT_MAX).contains(&l)
    {
        return Err(ValidationError::invalid(
            "limit",
            format!("limit must be 1–{PAGE_LIMIT_MAX}"),
        ));
    }
    Ok(())
}

/// A space membership (S10 §8). `role_ids` never includes `@everyone`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SpaceMember {
    /// The space.
    pub space_id: Id,
    /// The member.
    pub member_id: Id,
    /// When they joined.
    pub joined_at: Timestamp,
    /// Their roles, ascending; never `@everyone`.
    pub role_ids: Vec<Id>,
    /// End of an active timeout; present only while it is in the future (S12 §7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_until: Option<Timestamp>,
}

/// A page of space members (`GET /api/v1/spaces/{id}/members`).
pub type SpaceMemberPage = Page<SpaceMember>;

/// Query of `GET /api/v1/spaces/{id}/members`: a page forward by member id (`after`, `limit`),
/// or a batch lookup (`ids`, Hearth HC5). With `ids` the answer holds, ascending, only the ids
/// that are space members the caller may list (the page's view rules), with `has_more: false`;
/// other ids are left out silently. `ids` with any cursor or `limit` is the server's
/// `400 validation_failed` (field `ids`), so a full fixture can set every field.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpaceMembersQuery {
    /// Refused (`400`): this list pages forward with `after` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Id>,
    /// Members with a larger id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Id>,
    /// Refused (`400`): this list pages forward with `after` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub around: Option<Id>,
    /// Page size, 1–100 (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// 1–100 distinct member ids (HC5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<crate::id::IdList>,
}

impl SpaceMembersQuery {
    /// The page part of the query.
    pub fn page(&self) -> crate::http::PageQuery {
        crate::http::PageQuery {
            before: self.before,
            after: self.after,
            around: self.around,
            limit: self.limit,
        }
    }
}

impl Validate for SpaceMembersQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        // Cursor and `ids` combinations are the server's checks (see the type docs).
        check_page_limit(self.limit)
    }
}

/// A page of spaces (`GET /api/v1/spaces`).
pub type SpacePage = Page<Space>;

/// Body of `POST /api/v1/spaces/{id}/members/add`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddSpaceMembersRequest {
    /// 1–100 distinct member ids.
    pub member_ids: Vec<Id>,
}

impl Validate for AddSpaceMembersRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        check_unique_ids("member_ids", &self.member_ids, 1, 100)
    }
}

/// Response of `POST /api/v1/spaces/{id}/members/add`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AddSpaceMembersResponse {
    /// The memberships created.
    pub added: Vec<SpaceMember>,
    /// Ids that already were members.
    pub already: Vec<Id>,
}

/// Body of `POST /api/v1/spaces/{id}/members/remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveSpaceMemberRequest {
    /// The member to remove.
    pub member_id: Id,
}

impl Validate for RemoveSpaceMemberRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// Body of `POST /api/v1/spaces/{id}/transfer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransferSpaceRequest {
    /// The new owner.
    pub member_id: Id,
}

impl Validate for TransferSpaceRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

/// One category's new position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReorderCategoryItem {
    /// The category.
    pub category_id: Id,
    /// Its new position.
    pub position: u32,
}

/// Body of `POST /api/v1/spaces/{id}/categories/reorder`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReorderCategoriesRequest {
    /// 1–200 items with distinct category ids.
    pub items: Vec<ReorderCategoryItem>,
}

impl Validate for ReorderCategoriesRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        let ids: Vec<Id> = self.items.iter().map(|i| i.category_id).collect();
        check_unique_ids("items", &ids, 1, 200)
    }
}

/// The whole ordered category list of a space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CategoriesResponse {
    /// Every category, by position.
    pub categories: Vec<Category>,
}
