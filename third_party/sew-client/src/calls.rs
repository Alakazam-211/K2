//! S34 v2 calls: DM and group-DM calls (start or join, decline, re-ring and the three reads) and
//! the camera and screen-share leases on `POST /api/v1/me/voice/state` (prd-sew-34 v2 §14, §15.4,
//! §20.3, §23).
//!
//! Two different call ids (S34-m6): [`CallJoinResponse::call`]`.id` and `voice.call_id` are the
//! DM call (`calls.id`), which these methods take; `voice.e2ee.call_id` is S50's key-group scope
//! (the engine room), which the key methods in `voice.rs` take as a [`sew_proto::e2ee::CallId`].

use sew_proto::calls::{
    Call, CallDeclineRequest, CallJoinRequest, CallJoinResponse, CallRingRequest, ChannelCall,
    MyCalls,
};
use sew_proto::http::OkResponse;
use sew_proto::id::Id;
use sew_proto::routes::{self as r, with_call_id, with_id};
use sew_proto::voice::{VoiceSelfStateRequest, VoiceState};

use crate::{Client, ClientError};

impl Client {
    /// `POST /api/v1/channels/{id}/call/join`: starts the DM's call (ringing the others unless
    /// `ring: false`) or joins the active one. The body carries S50's `e2ee` object, made by an
    /// enrolled device from a KeyPackage scoped `call:<dm>`.
    pub async fn call_join(
        &self,
        dm: Id,
        req: &CallJoinRequest,
    ) -> Result<CallJoinResponse, ClientError> {
        self.post(&with_id(r::CHANNEL_CALL_JOIN, dm), req).await
    }

    /// `POST /api/v1/channels/{id}/call/decline`: declines the caller's pending ring of `call`.
    pub async fn call_decline(&self, dm: Id, call: Id) -> Result<OkResponse, ClientError> {
        let req = CallDeclineRequest { call_id: call };
        self.post(&with_id(r::CHANNEL_CALL_DECLINE, dm), &req).await
    }

    /// `POST /api/v1/channels/{id}/call/ring`: rings `members` again (`None`: every ringable
    /// member not in the call and not ringing). The caller must be in the call.
    pub async fn call_ring(
        &self,
        dm: Id,
        call: Id,
        members: Option<Vec<Id>>,
    ) -> Result<Call, ClientError> {
        let req = CallRingRequest {
            call_id: call,
            member_ids: members,
        };
        self.post(&with_id(r::CHANNEL_CALL_RING, dm), &req).await
    }

    /// `GET /api/v1/channels/{id}/call`: the DM's active call, if any.
    pub async fn call_get(&self, dm: Id) -> Result<ChannelCall, ClientError> {
        self.get(&with_id(r::CHANNEL_CALL, dm)).await
    }

    /// `GET /api/v1/me/calls`: active calls in the caller's DMs.
    pub async fn my_calls(&self) -> Result<MyCalls, ClientError> {
        self.get(r::ME_CALLS).await
    }

    /// `GET /api/v1/calls/{call_id}`: a call, active or ended (members of its DM only).
    pub async fn call_by_id(&self, call: Id) -> Result<Call, ClientError> {
        self.get(&with_call_id(r::CALL_BY_ID, &call.to_string()))
            .await
    }

    /// `POST /api/v1/me/voice/state` with `self_video`: asks for (`true`) or releases (`false`)
    /// a camera lease. Publish the camera only after the `200`.
    pub async fn voice_set_video(&self, on: bool) -> Result<VoiceState, ClientError> {
        let req = VoiceSelfStateRequest {
            self_video: Some(on),
            ..Default::default()
        };
        self.voice_self_state(&req).await
    }

    /// `POST /api/v1/me/voice/state` with `self_stream`: asks for (`true`) or releases (`false`)
    /// a screen-share lease. Publish the screen only after the `200`.
    pub async fn voice_set_stream(&self, on: bool) -> Result<VoiceState, ClientError> {
        let req = VoiceSelfStateRequest {
            self_stream: Some(on),
            ..Default::default()
        };
        self.voice_self_state(&req).await
    }
}
