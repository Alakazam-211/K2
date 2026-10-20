//! S10 calls: install roles, spaces, categories, channels, membership, DMs, resolve
//! (prd-sew-10 §9).

use sew_proto::addr::Addr;
use sew_proto::api::channels::{
    AddChannelMembersRequest, AddChannelMembersResponse, Channel, ChannelListQuery,
    ChannelListScope, ChannelMember, CreateChannelRequest, MyChannelsKind, MyChannelsQuery,
    OpenDmRequest, RemoveChannelMemberRequest, ReorderChannelsRequest, ReorderChannelsResponse,
    UpdateChannelRequest,
};
use sew_proto::api::guests::{GuestConvertResponse, GuestListQuery, GuestSummary, SetGuestRequest};
use sew_proto::api::members::{Member, SetInstallRoleRequest, SettableInstallRole};
use sew_proto::api::resolve::ResolveResponse;
use sew_proto::api::spaces::{
    AddSpaceMembersRequest, AddSpaceMembersResponse, CategoriesResponse, Category,
    CreateCategoryRequest, CreateSpaceRequest, RemoveSpaceMemberRequest, ReorderCategoriesRequest,
    Space, SpaceListQuery, SpaceListScope, SpaceMember, TransferSpaceRequest,
    UpdateCategoryRequest, UpdateSpaceRequest,
};
use sew_proto::http::{EmptyRequest, OkResponse, Page, PageQuery};
use sew_proto::id::{Id, IdList};
use sew_proto::routes::{self as r, with_id};

use crate::{Client, ClientError};

/// Percent-encodes a query value (RFC 3986 unreserved characters stay).
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `path?k=v&…` for the set pairs.
pub(crate) fn with_query(path: &str, pairs: &[(&str, Option<String>)]) -> String {
    let q: Vec<String> = pairs
        .iter()
        .filter_map(|(k, v)| v.as_ref().map(|v| format!("{k}={}", enc(v))))
        .collect();
    if q.is_empty() {
        path.to_owned()
    } else {
        format!("{path}?{}", q.join("&"))
    }
}

pub(crate) fn page_pairs(q: &PageQuery) -> Vec<(&'static str, Option<String>)> {
    vec![
        ("before", q.before.map(|v| v.to_string())),
        ("after", q.after.map(|v| v.to_string())),
        ("around", q.around.map(|v| v.to_string())),
        ("limit", q.limit.map(|v| v.to_string())),
    ]
}

impl Client {
    // ---- install

    /// `POST /api/v1/install/members/{id}/role` (install owner only).
    pub async fn set_install_role(
        &self,
        member: Id,
        role: SettableInstallRole,
    ) -> Result<Member, ClientError> {
        self.post(
            &with_id(r::INSTALL_MEMBER_ROLE, member),
            &SetInstallRoleRequest { install_role: role },
        )
        .await
    }

    /// `POST /api/v1/install/members/{id}/guest` (S19, standalone only): member ↔ guest.
    pub async fn set_guest(
        &self,
        member: Id,
        guest: bool,
    ) -> Result<GuestConvertResponse, ClientError> {
        self.post(
            &with_id(r::INSTALL_MEMBER_GUEST, member),
            &SetGuestRequest { guest },
        )
        .await
    }

    /// `GET /api/v1/install/guests` (S19, install owner and admins).
    pub async fn list_guests(&self, q: &GuestListQuery) -> Result<Page<GuestSummary>, ClientError> {
        self.get(&with_query(
            r::INSTALL_GUESTS,
            &[
                ("after", q.after.map(|v| v.to_string())),
                ("limit", q.limit.map(|v| v.to_string())),
                (
                    "include_deactivated",
                    q.include_deactivated.map(|v| v.to_string()),
                ),
            ],
        ))
        .await
    }

    // ---- spaces

    /// `GET /api/v1/spaces`.
    pub async fn spaces(&self, q: &SpaceListQuery) -> Result<Page<Space>, ClientError> {
        let scope = q.scope.map(|s: SpaceListScope| s.as_str().to_owned());
        self.get(&with_query(
            r::SPACES,
            &[
                ("scope", scope),
                ("after", q.after.map(|v| v.to_string())),
                ("limit", q.limit.map(|v| v.to_string())),
            ],
        ))
        .await
    }

    /// `POST /api/v1/spaces`.
    pub async fn create_space(&self, req: &CreateSpaceRequest) -> Result<Space, ClientError> {
        self.post(r::SPACES, req).await
    }

    /// `GET /api/v1/spaces/{id}`.
    pub async fn space(&self, id: Id) -> Result<Space, ClientError> {
        self.get(&with_id(r::SPACE, id)).await
    }

    /// `POST /api/v1/spaces/{id}/update`.
    pub async fn update_space(
        &self,
        id: Id,
        req: &UpdateSpaceRequest,
    ) -> Result<Space, ClientError> {
        self.post(&with_id(r::SPACE_UPDATE, id), req).await
    }

    /// `POST /api/v1/spaces/{id}/join`.
    pub async fn join_space(&self, id: Id) -> Result<Space, ClientError> {
        self.post(&with_id(r::SPACE_JOIN, id), &EmptyRequest {})
            .await
    }

    /// `POST /api/v1/spaces/{id}/leave`.
    pub async fn leave_space(&self, id: Id) -> Result<OkResponse, ClientError> {
        self.post(&with_id(r::SPACE_LEAVE, id), &EmptyRequest {})
            .await
    }

    /// `GET /api/v1/spaces/{id}/members`.
    pub async fn space_members(
        &self,
        id: Id,
        q: &PageQuery,
    ) -> Result<Page<SpaceMember>, ClientError> {
        self.get(&with_query(&with_id(r::SPACE_MEMBERS, id), &page_pairs(q)))
            .await
    }

    /// `GET /api/v1/spaces/{id}/members?ids=` (Hearth HC5): the listed members the caller may
    /// see in the space, ascending; ids that are not members are left out.
    pub async fn space_members_by_ids(
        &self,
        id: Id,
        ids: &IdList,
    ) -> Result<Page<SpaceMember>, ClientError> {
        self.get(&format!(
            "{}?ids={}",
            with_id(r::SPACE_MEMBERS, id),
            enc(&ids.to_string())
        ))
        .await
    }

    /// `POST /api/v1/spaces/{id}/members/add`.
    pub async fn add_space_members(
        &self,
        id: Id,
        member_ids: &[Id],
    ) -> Result<AddSpaceMembersResponse, ClientError> {
        self.post(
            &with_id(r::SPACE_MEMBERS_ADD, id),
            &AddSpaceMembersRequest {
                member_ids: member_ids.to_vec(),
            },
        )
        .await
    }

    /// `POST /api/v1/spaces/{id}/members/remove`.
    pub async fn remove_space_member(&self, id: Id, member: Id) -> Result<OkResponse, ClientError> {
        self.post(
            &with_id(r::SPACE_MEMBERS_REMOVE, id),
            &RemoveSpaceMemberRequest { member_id: member },
        )
        .await
    }

    /// `POST /api/v1/spaces/{id}/transfer`.
    pub async fn transfer_space(&self, id: Id, member: Id) -> Result<Space, ClientError> {
        self.post(
            &with_id(r::SPACE_TRANSFER, id),
            &TransferSpaceRequest { member_id: member },
        )
        .await
    }

    // ---- categories

    /// `GET /api/v1/spaces/{id}/categories`.
    pub async fn categories(&self, space: Id) -> Result<CategoriesResponse, ClientError> {
        self.get(&with_id(r::SPACE_CATEGORIES, space)).await
    }

    /// `POST /api/v1/spaces/{id}/categories`.
    pub async fn create_category(
        &self,
        space: Id,
        req: &CreateCategoryRequest,
    ) -> Result<Category, ClientError> {
        self.post(&with_id(r::SPACE_CATEGORIES, space), req).await
    }

    /// `POST /api/v1/categories/{id}/update`.
    pub async fn update_category(
        &self,
        id: Id,
        req: &UpdateCategoryRequest,
    ) -> Result<Category, ClientError> {
        self.post(&with_id(r::CATEGORY_UPDATE, id), req).await
    }

    /// `POST /api/v1/categories/{id}/delete`.
    pub async fn delete_category(&self, id: Id) -> Result<OkResponse, ClientError> {
        self.post(&with_id(r::CATEGORY_DELETE, id), &EmptyRequest {})
            .await
    }

    /// `POST /api/v1/spaces/{id}/categories/reorder`.
    pub async fn reorder_categories(
        &self,
        space: Id,
        req: &ReorderCategoriesRequest,
    ) -> Result<CategoriesResponse, ClientError> {
        self.post(&with_id(r::SPACE_CATEGORIES_REORDER, space), req)
            .await
    }

    // ---- channels

    /// `GET /api/v1/spaces/{id}/channels`.
    pub async fn channels(
        &self,
        space: Id,
        q: &ChannelListQuery,
    ) -> Result<Page<Channel>, ClientError> {
        let scope = q.scope.map(|s: ChannelListScope| s.as_str().to_owned());
        self.get(&with_query(
            &with_id(r::SPACE_CHANNELS, space),
            &[
                ("scope", scope),
                (
                    "include_archived",
                    q.include_archived.then(|| "true".to_owned()),
                ),
                ("q", q.q.clone()),
                ("after", q.after.map(|v| v.to_string())),
                ("limit", q.limit.map(|v| v.to_string())),
            ],
        ))
        .await
    }

    /// `POST /api/v1/spaces/{id}/channels`.
    pub async fn create_channel(
        &self,
        space: Id,
        req: &CreateChannelRequest,
    ) -> Result<Channel, ClientError> {
        self.post(&with_id(r::SPACE_CHANNELS, space), req).await
    }

    /// `GET /api/v1/channels/{id}`.
    pub async fn channel(&self, id: Id) -> Result<Channel, ClientError> {
        self.get(&with_id(r::CHANNEL, id)).await
    }

    /// `POST /api/v1/channels/{id}/update`.
    pub async fn update_channel(
        &self,
        id: Id,
        req: &UpdateChannelRequest,
    ) -> Result<Channel, ClientError> {
        self.post(&with_id(r::CHANNEL_UPDATE, id), req).await
    }

    /// `POST /api/v1/channels/{id}/archive`.
    pub async fn archive_channel(&self, id: Id) -> Result<Channel, ClientError> {
        self.post(&with_id(r::CHANNEL_ARCHIVE, id), &EmptyRequest {})
            .await
    }

    /// `POST /api/v1/channels/{id}/unarchive`.
    pub async fn unarchive_channel(&self, id: Id) -> Result<Channel, ClientError> {
        self.post(&with_id(r::CHANNEL_UNARCHIVE, id), &EmptyRequest {})
            .await
    }

    /// `POST /api/v1/spaces/{id}/channels/reorder`.
    pub async fn reorder_channels(
        &self,
        space: Id,
        req: &ReorderChannelsRequest,
    ) -> Result<ReorderChannelsResponse, ClientError> {
        self.post(&with_id(r::SPACE_CHANNELS_REORDER, space), req)
            .await
    }

    /// `GET /api/v1/me/channels`.
    pub async fn my_channels(&self, q: &MyChannelsQuery) -> Result<Page<Channel>, ClientError> {
        let kind = q.kind.map(|k: MyChannelsKind| k.as_str().to_owned());
        self.get(&with_query(
            r::ME_CHANNELS,
            &[
                ("kind", kind),
                ("after", q.after.map(|v| v.to_string())),
                ("limit", q.limit.map(|v| v.to_string())),
            ],
        ))
        .await
    }

    // ---- membership

    /// `POST /api/v1/channels/{id}/join`.
    pub async fn join_channel(&self, id: Id) -> Result<Channel, ClientError> {
        self.post(&with_id(r::CHANNEL_JOIN, id), &EmptyRequest {})
            .await
    }

    /// `POST /api/v1/channels/{id}/leave`.
    pub async fn leave_channel(&self, id: Id) -> Result<OkResponse, ClientError> {
        self.post(&with_id(r::CHANNEL_LEAVE, id), &EmptyRequest {})
            .await
    }

    /// `GET /api/v1/channels/{id}/members`.
    pub async fn channel_members(
        &self,
        id: Id,
        q: &PageQuery,
    ) -> Result<Page<ChannelMember>, ClientError> {
        self.get(&with_query(
            &with_id(r::CHANNEL_MEMBERS, id),
            &page_pairs(q),
        ))
        .await
    }

    /// `POST /api/v1/channels/{id}/members/add`.
    pub async fn add_channel_members(
        &self,
        id: Id,
        member_ids: &[Id],
    ) -> Result<AddChannelMembersResponse, ClientError> {
        self.post(
            &with_id(r::CHANNEL_MEMBERS_ADD, id),
            &AddChannelMembersRequest {
                member_ids: member_ids.to_vec(),
            },
        )
        .await
    }

    /// `POST /api/v1/channels/{id}/members/remove`.
    pub async fn remove_channel_member(
        &self,
        id: Id,
        member: Id,
    ) -> Result<OkResponse, ClientError> {
        self.post(
            &with_id(r::CHANNEL_MEMBERS_REMOVE, id),
            &RemoveChannelMemberRequest { member_id: member },
        )
        .await
    }

    // ---- DMs and resolve

    /// `POST /api/v1/dms`: find or create the DM or group DM with `member_ids`.
    pub async fn open_dm(&self, member_ids: &[Id]) -> Result<Channel, ClientError> {
        self.post(
            r::DMS,
            &OpenDmRequest {
                member_ids: member_ids.to_vec(),
                usernames: vec![],
            },
        )
        .await
    }

    /// `POST /api/v1/dms` naming the others by id and/or handle (SW12, additive): 1–8
    /// distinct members in all. An unknown or hidden handle is a masked `404` with
    /// `details.username`.
    pub async fn open_dm_with(&self, req: &OpenDmRequest) -> Result<Channel, ClientError> {
        self.post(r::DMS, req).await
    }

    /// `GET /api/v1/resolve?addr=` (`#` sent as `%23`).
    pub async fn resolve(&self, addr: &Addr) -> Result<ResolveResponse, ClientError> {
        self.get(&format!("{}?addr={}", r::RESOLVE, addr.to_query_value()))
            .await
    }
}
