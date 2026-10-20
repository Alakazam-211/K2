//! S18 mgmt calls (prd-sew-18 §16): the K2 avatar push and clear, provider custom field
//! definitions.

use bytes::Bytes;
use http::Method;
use sew_proto::api::people::{
    MgmtAvatarClearRequest, MgmtAvatarQuery, MgmtAvatarResponse, ProfileField,
    ProfileFieldsResponse, UpsertProfileFieldRequest,
};
use sew_proto::routes::{
    MGMT_PRINCIPAL_AVATAR, MGMT_PRINCIPAL_AVATAR_CLEAR, MGMT_PROFILE_FIELDS,
    MGMT_PROFILE_FIELDS_UPSERT,
};

use super::MgmtClient;
use crate::api::{decode, with_query};
use crate::config::RequestOptions;
use crate::error::ClientError;

impl MgmtClient {
    /// `POST /mgmt/v1/principals/avatar` with the raw image: kept when the member chose their
    /// own avatar unless `force` (`applied: false, reason: member_avatar_kept`).
    pub async fn principal_avatar(
        &self,
        q: &MgmtAvatarQuery,
        image: Vec<u8>,
        opts: RequestOptions,
    ) -> Result<MgmtAvatarResponse, ClientError> {
        let path = with_query(MGMT_PRINCIPAL_AVATAR, Some(q))?;
        let (resp, _) = self
            .client
            .execute(Method::POST, path, Some(Bytes::from(image)), opts)
            .await?;
        decode(resp.status, &resp.body)
    }

    /// `POST /mgmt/v1/principals/avatar/clear`: clears a provider avatar (any with `force`).
    pub async fn principal_avatar_clear(
        &self,
        req: &MgmtAvatarClearRequest,
        opts: RequestOptions,
    ) -> Result<MgmtAvatarResponse, ClientError> {
        self.client
            .post_with(MGMT_PRINCIPAL_AVATAR_CLEAR, req, opts)
            .await
    }

    /// `GET /mgmt/v1/profile-fields`: every live custom field definition.
    pub async fn profile_fields(&self) -> Result<ProfileFieldsResponse, ClientError> {
        self.client.get(MGMT_PROFILE_FIELDS).await
    }

    /// `POST /mgmt/v1/profile-fields/upsert`: create or update by `key`. Errors: 422
    /// `field_kind_fixed`, 403 `quota_exceeded`.
    pub async fn profile_field_upsert(
        &self,
        req: &UpsertProfileFieldRequest,
        opts: RequestOptions,
    ) -> Result<ProfileField, ClientError> {
        self.client
            .post_with(MGMT_PROFILE_FIELDS_UPSERT, req, opts)
            .await
    }
}
