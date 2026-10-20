//! S18 calls: profiles, avatars, status, presence and DND, the directory, custom fields, user
//! groups and mentionables (prd-sew-18 §15).

use sew_proto::api::members::Member;
use sew_proto::api::people::{
    AddGroupMembersResponse, CreateProfileFieldRequest, CreateUserGroupRequest, DirectoryPage,
    DirectoryQuery, GroupMember, GroupMembersRequest, MembersQuery, MentionablesQuery,
    MentionablesResponse, PresenceList, PresenceSettings, ProfileField, ProfileFieldsResponse,
    RemoveGroupMembersResponse, ReorderProfileFieldsRequest, ResetProfileRequest,
    SetDndScheduleRequest, SetPresenceRequest, SetStatusRequest, UpdateMyProfileRequest,
    UpdateProfileFieldRequest, UpdateUserGroupRequest, UserGroup, UserGroupsResponse,
};
use sew_proto::http::{EmptyRequest, OkResponse, Page, PageQuery};
use sew_proto::id::Id;
use sew_proto::routes::{self as r, with_id};

use bytes::Bytes;
use http::Method;

use crate::api::decode;
use crate::config::RequestOptions;
use crate::structure::{page_pairs, with_query};
use crate::{Client, ClientError, RawResponse};

impl Client {
    // ---- profiles

    /// `GET /api/v1/members/{id}`.
    pub async fn member(&self, id: Id) -> Result<Member, ClientError> {
        self.get(&with_id(r::MEMBER, id)).await
    }

    /// `GET /api/v1/members?ids=` or `?username=`.
    pub async fn members(&self, q: &MembersQuery) -> Result<Page<Member>, ClientError> {
        let path = with_query(
            r::MEMBERS,
            &[
                ("ids", q.ids.as_ref().map(|l| l.to_string())),
                (
                    "username",
                    q.username.as_ref().map(|h| h.as_str().to_owned()),
                ),
            ],
        );
        self.get(&path).await
    }

    /// `POST /api/v1/me/profile`.
    pub async fn update_my_profile(
        &self,
        req: &UpdateMyProfileRequest,
    ) -> Result<Member, ClientError> {
        self.post(r::ME_PROFILE, req).await
    }

    /// `POST /api/v1/me/avatar` with a raw image body.
    pub async fn upload_avatar(&self, image: Vec<u8>) -> Result<Member, ClientError> {
        let (resp, _) = self
            .execute(
                Method::POST,
                r::ME_AVATAR.to_owned(),
                Some(Bytes::from(image)),
                RequestOptions::default(),
            )
            .await?;
        decode(resp.status, &resp.body)
    }

    /// `POST /api/v1/me/avatar/clear`.
    pub async fn clear_avatar(&self) -> Result<Member, ClientError> {
        self.post(r::ME_AVATAR_CLEAR, &EmptyRequest {}).await
    }

    /// `GET` a signed avatar URL (`/api/v1/avatars/{member_id}/{avatar_id}/{size}?exp=&sig=`,
    /// relative to the client's base, as `Member.avatar_url` carries it); the raw response. The
    /// URL is the credential, so no Bearer is sent; an expired or bad signature is a 403
    /// `url_expired` / `url_invalid` (rev 28).
    pub async fn fetch_avatar(&self, url: &str) -> Result<RawResponse, ClientError> {
        let prefix = r::AVATAR.split('{').next().unwrap_or(r::AVATAR);
        if !url.starts_with(prefix) {
            return Err(ClientError::invalid(
                "url",
                format!("an avatar URL starts with {prefix}"),
            ));
        }
        self.request("GET", url, &[("authorization", "")], None)
            .await
    }

    /// `POST /api/v1/me/status`.
    pub async fn set_status(&self, req: &SetStatusRequest) -> Result<Member, ClientError> {
        self.post(r::ME_STATUS, req).await
    }

    /// `POST /api/v1/me/status/clear`.
    pub async fn clear_status(&self) -> Result<Member, ClientError> {
        self.post(r::ME_STATUS_CLEAR, &EmptyRequest {}).await
    }

    /// `POST /api/v1/members/{id}/profile/reset`.
    pub async fn reset_profile(
        &self,
        id: Id,
        req: &ResetProfileRequest,
    ) -> Result<Member, ClientError> {
        self.post(&with_id(r::MEMBER_PROFILE_RESET, id), req).await
    }

    // ---- presence and DND

    /// `GET /api/v1/me/presence`.
    pub async fn my_presence(&self) -> Result<PresenceSettings, ClientError> {
        self.get(r::ME_PRESENCE).await
    }

    /// `POST /api/v1/me/presence`.
    pub async fn set_presence(
        &self,
        req: &SetPresenceRequest,
    ) -> Result<PresenceSettings, ClientError> {
        self.post(r::ME_PRESENCE, req).await
    }

    /// `POST /api/v1/me/dnd/schedule`.
    pub async fn set_dnd_schedule(
        &self,
        req: &SetDndScheduleRequest,
    ) -> Result<PresenceSettings, ClientError> {
        self.post(r::ME_DND_SCHEDULE, req).await
    }

    /// `POST /api/v1/me/dnd/end`.
    pub async fn end_dnd(&self) -> Result<PresenceSettings, ClientError> {
        self.post(r::ME_DND_END, &EmptyRequest {}).await
    }

    /// `GET /api/v1/presence?ids=`.
    pub async fn presence(&self, ids: &[Id]) -> Result<PresenceList, ClientError> {
        let list: Vec<String> = ids.iter().map(Id::to_string).collect();
        self.get(&with_query(r::PRESENCE, &[("ids", Some(list.join(",")))]))
            .await
    }

    // ---- directory

    /// `GET /api/v1/directory`.
    pub async fn directory(&self, q: &DirectoryQuery) -> Result<DirectoryPage, ClientError> {
        let path = with_query(
            r::DIRECTORY,
            &[
                ("q", q.q.clone()),
                ("space_id", q.space_id.map(|v| v.to_string())),
                ("channel_id", q.channel_id.map(|v| v.to_string())),
                ("kind", q.kind.map(|k| k.as_str().to_owned())),
                ("cursor", q.cursor.clone()),
                ("limit", q.limit.map(|v| v.to_string())),
            ],
        );
        self.get(&path).await
    }

    // ---- custom fields

    /// `GET /api/v1/install/profile-fields`.
    pub async fn profile_fields(&self) -> Result<ProfileFieldsResponse, ClientError> {
        self.get(r::INSTALL_PROFILE_FIELDS).await
    }

    /// `POST /api/v1/install/profile-fields`.
    pub async fn create_profile_field(
        &self,
        req: &CreateProfileFieldRequest,
    ) -> Result<ProfileField, ClientError> {
        self.post(r::INSTALL_PROFILE_FIELDS, req).await
    }

    /// `POST /api/v1/install/profile-fields/{id}/update`.
    pub async fn update_profile_field(
        &self,
        id: Id,
        req: &UpdateProfileFieldRequest,
    ) -> Result<ProfileField, ClientError> {
        self.post(&with_id(r::INSTALL_PROFILE_FIELD_UPDATE, id), req)
            .await
    }

    /// `POST /api/v1/install/profile-fields/{id}/delete`.
    pub async fn delete_profile_field(&self, id: Id) -> Result<OkResponse, ClientError> {
        self.post(
            &with_id(r::INSTALL_PROFILE_FIELD_DELETE, id),
            &EmptyRequest {},
        )
        .await
    }

    /// `POST /api/v1/install/profile-fields/reorder`.
    pub async fn reorder_profile_fields(
        &self,
        req: &ReorderProfileFieldsRequest,
    ) -> Result<ProfileFieldsResponse, ClientError> {
        self.post(r::INSTALL_PROFILE_FIELDS_REORDER, req).await
    }

    // ---- groups

    /// `GET /api/v1/groups?space_id=`.
    pub async fn groups(&self, space_id: Option<Id>) -> Result<UserGroupsResponse, ClientError> {
        self.get(&with_query(
            r::GROUPS,
            &[("space_id", space_id.map(|v| v.to_string()))],
        ))
        .await
    }

    /// `GET /api/v1/groups/{id}`.
    pub async fn group(&self, id: Id) -> Result<UserGroup, ClientError> {
        self.get(&with_id(r::GROUP, id)).await
    }

    /// `GET /api/v1/groups/{id}/members`.
    pub async fn group_members(
        &self,
        id: Id,
        q: &PageQuery,
    ) -> Result<Page<GroupMember>, ClientError> {
        self.get(&with_query(&with_id(r::GROUP_MEMBERS, id), &page_pairs(q)))
            .await
    }

    /// `POST /api/v1/groups`.
    pub async fn create_group(
        &self,
        req: &CreateUserGroupRequest,
    ) -> Result<UserGroup, ClientError> {
        self.post(r::GROUPS, req).await
    }

    /// `POST /api/v1/groups/{id}/update`.
    pub async fn update_group(
        &self,
        id: Id,
        req: &UpdateUserGroupRequest,
    ) -> Result<UserGroup, ClientError> {
        self.post(&with_id(r::GROUP_UPDATE, id), req).await
    }

    /// `POST /api/v1/groups/{id}/delete`.
    pub async fn delete_group(&self, id: Id) -> Result<OkResponse, ClientError> {
        self.post(&with_id(r::GROUP_DELETE, id), &EmptyRequest {})
            .await
    }

    /// `POST /api/v1/groups/{id}/members/add`.
    pub async fn add_group_members(
        &self,
        id: Id,
        members: &[Id],
    ) -> Result<AddGroupMembersResponse, ClientError> {
        self.post(
            &with_id(r::GROUP_MEMBERS_ADD, id),
            &GroupMembersRequest {
                member_ids: members.to_vec(),
            },
        )
        .await
    }

    /// `POST /api/v1/groups/{id}/members/remove`.
    pub async fn remove_group_members(
        &self,
        id: Id,
        members: &[Id],
    ) -> Result<RemoveGroupMembersResponse, ClientError> {
        self.post(
            &with_id(r::GROUP_MEMBERS_REMOVE, id),
            &GroupMembersRequest {
                member_ids: members.to_vec(),
            },
        )
        .await
    }

    /// `GET /api/v1/channels/{id}/mentionables`.
    pub async fn mentionables(
        &self,
        channel: Id,
        q: &MentionablesQuery,
    ) -> Result<MentionablesResponse, ClientError> {
        let path = with_query(
            &with_id(r::CHANNEL_MENTIONABLES, channel),
            &[
                ("q", q.q.clone()),
                ("limit", q.limit.map(|v| v.to_string())),
            ],
        );
        self.get(&path).await
    }
}
