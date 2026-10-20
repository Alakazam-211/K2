//! S12 calls: roles, member roles, overwrites, channel sync, timeouts, slow mode, install
//! policies (prd-sew-12 §8).

use sew_proto::api::channels::Channel;
use sew_proto::api::moderation::{SlowModeRequest, TimeoutRequest};
use sew_proto::api::overwrites::{ChannelOverwrites, OverwriteRef, SetOverwriteRequest};
use sew_proto::api::roles::{
    CreateRoleRequest, MemberRolesRequest, ReorderRolesRequest, RolePage, UpdateRoleRequest,
};
use sew_proto::api::spaces::{Category, OverwriteTarget, Role, SpaceMember, SpaceMemberPage};
use sew_proto::http::{EmptyRequest, OkResponse, PageQuery};
use sew_proto::id::Id;
use sew_proto::perms::Perms;
use sew_proto::policy::{PoliciesResponse, PolicyPatch, SetPoliciesRequest};
use sew_proto::routes::{self as r, with_id, with_ids};

use crate::structure::{page_pairs, with_query};
use crate::{Client, ClientError, RawResponse};

impl Client {
    // ---- roles (§8.1)

    /// `GET /api/v1/spaces/{id}/roles`.
    pub async fn roles(&self, space: Id) -> Result<RolePage, ClientError> {
        self.get(&with_id(r::SPACE_ROLES, space)).await
    }

    /// `GET /api/v1/spaces/{id}/roles` with `If-None-Match` (the raw response: `200` with an
    /// `ETag`, or `304`).
    pub async fn roles_if_none_match(
        &self,
        space: Id,
        etag: &str,
    ) -> Result<RawResponse, ClientError> {
        self.request(
            "GET",
            &with_id(r::SPACE_ROLES, space),
            &[("if-none-match", etag)],
            None,
        )
        .await
    }

    /// `POST /api/v1/spaces/{id}/roles`.
    pub async fn create_role(
        &self,
        space: Id,
        req: &CreateRoleRequest,
    ) -> Result<Role, ClientError> {
        self.post(&with_id(r::SPACE_ROLES, space), req).await
    }

    /// `POST /api/v1/roles/{id}/edit`.
    pub async fn edit_role(&self, role: Id, req: &UpdateRoleRequest) -> Result<Role, ClientError> {
        self.post(&with_id(r::ROLE_EDIT, role), req).await
    }

    /// `POST /api/v1/roles/{id}/delete`.
    pub async fn delete_role(&self, role: Id) -> Result<OkResponse, ClientError> {
        self.post(&with_id(r::ROLE_DELETE, role), &EmptyRequest {})
            .await
    }

    /// `POST /api/v1/spaces/{id}/roles/reorder` (every non-`@everyone` role, bottom to top).
    pub async fn reorder_roles(&self, space: Id, role_ids: &[Id]) -> Result<RolePage, ClientError> {
        self.post(
            &with_id(r::SPACE_ROLES_REORDER, space),
            &ReorderRolesRequest {
                role_ids: role_ids.to_vec(),
            },
        )
        .await
    }

    /// `GET /api/v1/roles/{id}/members`.
    pub async fn role_members(
        &self,
        role: Id,
        q: &PageQuery,
    ) -> Result<SpaceMemberPage, ClientError> {
        self.get(&with_query(&with_id(r::ROLE_MEMBERS, role), &page_pairs(q)))
            .await
    }

    // ---- member roles (§8.2)

    /// `GET /api/v1/spaces/{id}/members/{member_id}`.
    pub async fn space_member(&self, space: Id, member: Id) -> Result<SpaceMember, ClientError> {
        self.get(&with_ids(r::SPACE_MEMBER, space, member)).await
    }

    /// `POST /api/v1/spaces/{id}/members/{member_id}/roles/add`.
    pub async fn add_member_roles(
        &self,
        space: Id,
        member: Id,
        role_ids: &[Id],
    ) -> Result<SpaceMember, ClientError> {
        self.post(
            &with_ids(r::SPACE_MEMBER_ROLES_ADD, space, member),
            &MemberRolesRequest {
                role_ids: role_ids.to_vec(),
            },
        )
        .await
    }

    /// `POST /api/v1/spaces/{id}/members/{member_id}/roles/remove`.
    pub async fn remove_member_roles(
        &self,
        space: Id,
        member: Id,
        role_ids: &[Id],
    ) -> Result<SpaceMember, ClientError> {
        self.post(
            &with_ids(r::SPACE_MEMBER_ROLES_REMOVE, space, member),
            &MemberRolesRequest {
                role_ids: role_ids.to_vec(),
            },
        )
        .await
    }

    // ---- overwrites and sync (§8.3–§8.5)

    /// `GET /api/v1/channels/{id}/overwrites`.
    pub async fn channel_overwrites(&self, channel: Id) -> Result<ChannelOverwrites, ClientError> {
        self.get(&with_id(r::CHANNEL_OVERWRITES, channel)).await
    }

    /// `POST /api/v1/channels/{id}/overwrites/set`.
    pub async fn set_channel_overwrite(
        &self,
        channel: Id,
        target_kind: OverwriteTarget,
        target_id: Id,
        allow: Perms,
        deny: Perms,
    ) -> Result<ChannelOverwrites, ClientError> {
        self.post(
            &with_id(r::CHANNEL_OVERWRITES_SET, channel),
            &SetOverwriteRequest {
                target_kind,
                target_id,
                allow,
                deny,
            },
        )
        .await
    }

    /// `POST /api/v1/channels/{id}/overwrites/delete`.
    pub async fn delete_channel_overwrite(
        &self,
        channel: Id,
        target_kind: OverwriteTarget,
        target_id: Id,
    ) -> Result<ChannelOverwrites, ClientError> {
        self.post(
            &with_id(r::CHANNEL_OVERWRITES_DELETE, channel),
            &OverwriteRef {
                target_kind,
                target_id,
            },
        )
        .await
    }

    /// `POST /api/v1/channels/{id}/sync`.
    pub async fn sync_channel(&self, channel: Id) -> Result<ChannelOverwrites, ClientError> {
        self.post(&with_id(r::CHANNEL_SYNC, channel), &EmptyRequest {})
            .await
    }

    /// `GET /api/v1/categories/{id}/overwrites`.
    pub async fn category_overwrites(&self, category: Id) -> Result<Category, ClientError> {
        self.get(&with_id(r::CATEGORY_OVERWRITES, category)).await
    }

    /// `POST /api/v1/categories/{id}/overwrites/set`.
    pub async fn set_category_overwrite(
        &self,
        category: Id,
        target_kind: OverwriteTarget,
        target_id: Id,
        allow: Perms,
        deny: Perms,
    ) -> Result<Category, ClientError> {
        self.post(
            &with_id(r::CATEGORY_OVERWRITES_SET, category),
            &SetOverwriteRequest {
                target_kind,
                target_id,
                allow,
                deny,
            },
        )
        .await
    }

    /// `POST /api/v1/categories/{id}/overwrites/delete`.
    pub async fn delete_category_overwrite(
        &self,
        category: Id,
        target_kind: OverwriteTarget,
        target_id: Id,
    ) -> Result<Category, ClientError> {
        self.post(
            &with_id(r::CATEGORY_OVERWRITES_DELETE, category),
            &OverwriteRef {
                target_kind,
                target_id,
            },
        )
        .await
    }

    // ---- moderation (§8.6, §8.8)

    /// `POST /api/v1/spaces/{id}/members/{member_id}/timeout`.
    pub async fn timeout_member(
        &self,
        space: Id,
        member: Id,
        duration_ms: u64,
    ) -> Result<SpaceMember, ClientError> {
        self.post(
            &with_ids(r::SPACE_MEMBER_TIMEOUT, space, member),
            &TimeoutRequest { duration_ms },
        )
        .await
    }

    /// `POST /api/v1/spaces/{id}/members/{member_id}/timeout/clear`.
    pub async fn clear_timeout(&self, space: Id, member: Id) -> Result<SpaceMember, ClientError> {
        self.post(
            &with_ids(r::SPACE_MEMBER_TIMEOUT_CLEAR, space, member),
            &EmptyRequest {},
        )
        .await
    }

    /// `POST /api/v1/channels/{id}/slow_mode/set`.
    pub async fn set_slow_mode(
        &self,
        channel: Id,
        slow_mode_s: u32,
    ) -> Result<Channel, ClientError> {
        self.post(
            &with_id(r::CHANNEL_SLOW_MODE_SET, channel),
            &SlowModeRequest { slow_mode_s },
        )
        .await
    }

    // ---- install policies (§8.9)

    /// `GET /api/v1/install/policies`.
    pub async fn org_policies(&self) -> Result<PoliciesResponse, ClientError> {
        self.get(r::INSTALL_POLICIES).await
    }

    /// `POST /api/v1/install/policies`.
    pub async fn set_org_policies(
        &self,
        changes: PolicyPatch,
    ) -> Result<PoliciesResponse, ClientError> {
        self.post(r::INSTALL_POLICIES, &SetPoliciesRequest { changes })
            .await
    }
}
