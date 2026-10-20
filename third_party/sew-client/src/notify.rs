//! S21 calls: notification settings, channel and space prefs, and the signed digest unsubscribe
//! link (prd-sew-21 §12.1). Events `notification_created`, `notifications_released` and
//! `notification_prefs_updated` decode through `sew_proto::gateway::events::Event`.

use sew_proto::api::notify::{
    ChannelNotifyPrefs, NotifyPrefsList, NotifySettings, SetChannelNotifyRequest,
    SetSpaceNotifyRequest, SpaceNotifyPrefs, UpdateNotifySettingsRequest,
};
use sew_proto::id::Id;
use sew_proto::routes::{self as r, with_id};

use crate::{Client, ClientError};

impl Client {
    /// `GET /api/v1/me/notify`: the caller's settings, keywords included.
    pub async fn notify_settings(&self) -> Result<NotifySettings, ClientError> {
        self.get(r::ME_NOTIFY).await
    }

    /// `POST /api/v1/me/notify/set`. Errors: 400 (a field, `keywords[i]`,
    /// `keyword_is_mention`), 409 on an `if_rev` mismatch (`details.current_rev`).
    pub async fn set_notify_settings(
        &self,
        req: &UpdateNotifySettingsRequest,
    ) -> Result<NotifySettings, ClientError> {
        self.post(r::ME_NOTIFY_SET, req).await
    }

    /// `GET /api/v1/me/notify/prefs`: explicit channel and space rows the caller can see.
    pub async fn notify_prefs(&self) -> Result<NotifyPrefsList, ClientError> {
        self.get(r::ME_NOTIFY_PREFS).await
    }

    /// `GET /api/v1/channels/{id}/notify`. Errors: masked 404, 422 `not_a_member`.
    pub async fn channel_notify(&self, channel: Id) -> Result<ChannelNotifyPrefs, ClientError> {
        self.get(&with_id(r::CHANNEL_NOTIFY, channel)).await
    }

    /// `POST /api/v1/channels/{id}/notify/set`: level and/or mute. Errors: masked 404, 422
    /// `not_a_member`, 400 `mute.until`.
    pub async fn set_channel_notify(
        &self,
        channel: Id,
        req: &SetChannelNotifyRequest,
    ) -> Result<ChannelNotifyPrefs, ClientError> {
        self.post(&with_id(r::CHANNEL_NOTIFY_SET, channel), req)
            .await
    }

    /// `GET /api/v1/spaces/{id}/notify`. Errors: masked 404.
    pub async fn space_notify(&self, space: Id) -> Result<SpaceNotifyPrefs, ClientError> {
        self.get(&with_id(r::SPACE_NOTIFY, space)).await
    }

    /// `POST /api/v1/spaces/{id}/notify/set`. Errors: masked 404.
    pub async fn set_space_notify(
        &self,
        space: Id,
        req: &SetSpaceNotifyRequest,
    ) -> Result<SpaceNotifyPrefs, ClientError> {
        self.post(&with_id(r::SPACE_NOTIFY_SET, space), req).await
    }
}
