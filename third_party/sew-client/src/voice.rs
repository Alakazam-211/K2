//! S33 v2 and S50 calls: the voice join, leave and own state (prd-sew-33 v2 §18), the key
//! routes K2–K10 and K16 and the device routes K11–K15 (prd-sew-50 §9.2).

use serde_json::Value;
use sew_proto::e2ee::{
    CallId, CallKeysInit, CrlPage, CrlQuery, DeviceCertificateResponse, DeviceEnrollRequest,
    DevicePage, DeviceRevokeRequest, DeviceRevokeResponse, KeyCommitAccepted, KeyCommitRequest,
    KeyInvalidRequest, KeyJoinRequest, KeyPackageUploadRequest, KeyPackageUploadResponse,
    KeyReadyRequest, KeyRekeyRequest, KeyReleaseRequest, KeyReportRequest, KeyResetPackageRequest,
    KeyStateRequest, KeyStateResponse,
};
use sew_proto::http::{EmptyRequest, OkResponse, Patch};
use sew_proto::id::Id;
use sew_proto::routes::{self as r, with_call_id, with_id, with_ids};
use sew_proto::voice::{
    CallStatsSample, MeVoice, VoiceFlags, VoiceFlagsRequest, VoiceJoinRequest, VoiceJoinResponse,
    VoiceMoveRequest, VoiceSelfStateRequest, VoiceState, VoiceStatePage,
};

use crate::structure::with_query;
use crate::{Client, ClientError};

impl Client {
    // ---- S33 v2

    /// `POST /api/v1/channels/{id}/voice/join` (the body carries S50's `e2ee` object).
    pub async fn voice_join(
        &self,
        channel: Id,
        req: &VoiceJoinRequest,
    ) -> Result<VoiceJoinResponse, ClientError> {
        self.post(&with_id(r::CHANNEL_VOICE_JOIN, channel), req)
            .await
    }

    /// `POST /api/v1/me/voice/leave` (idempotent).
    pub async fn voice_leave(&self) -> Result<OkResponse, ClientError> {
        self.post(r::ME_VOICE_LEAVE, &EmptyRequest {}).await
    }

    /// `GET /api/v1/me/voice`.
    pub async fn my_voice(&self) -> Result<MeVoice, ClientError> {
        self.get(r::ME_VOICE).await
    }

    /// `POST /api/v1/me/voice/state`: self mute and deafen (`self_deaf` forces `self_mute`).
    pub async fn voice_self_state(
        &self,
        req: &VoiceSelfStateRequest,
    ) -> Result<VoiceState, ClientError> {
        self.post(r::ME_VOICE_STATE, req).await
    }

    /// `POST /api/v1/me/voice/stats` (F21): one aggregate call-stats sample of the caller's
    /// current call, while the join answered `stats` (`calls.stats_beacon` on).
    pub async fn voice_stats(&self, req: &CallStatsSample) -> Result<OkResponse, ClientError> {
        self.post(r::ME_VOICE_STATS, req).await
    }

    /// `POST /api/v1/spaces/{id}/members/{member_id}/voice`: server mute / deafen.
    pub async fn voice_flags(
        &self,
        space: Id,
        member: Id,
        req: &VoiceFlagsRequest,
    ) -> Result<VoiceFlags, ClientError> {
        self.post(&with_ids(r::SPACE_MEMBER_VOICE, space, member), req)
            .await
    }

    /// `POST /api/v1/spaces/{id}/members/{member_id}/voice/move` to `channel`: the member's new
    /// state.
    pub async fn voice_move(
        &self,
        space: Id,
        member: Id,
        channel: Id,
    ) -> Result<VoiceState, ClientError> {
        let req = VoiceMoveRequest {
            channel_id: Patch::Set(channel),
        };
        self.post(&with_ids(r::SPACE_MEMBER_VOICE_MOVE, space, member), &req)
            .await
    }

    /// The same route with `channel_id: null`: disconnects the member.
    pub async fn voice_disconnect(&self, space: Id, member: Id) -> Result<OkResponse, ClientError> {
        let req = VoiceMoveRequest {
            channel_id: Patch::Clear,
        };
        self.post(&with_ids(r::SPACE_MEMBER_VOICE_MOVE, space, member), &req)
            .await
    }

    /// `GET /api/v1/spaces/{id}/voice/states`.
    pub async fn space_voice_states(&self, space: Id) -> Result<VoiceStatePage, ClientError> {
        self.get(&with_id(r::SPACE_VOICE_STATES, space)).await
    }

    /// `GET /api/v1/channels/{id}/voice/states`.
    pub async fn channel_voice_states(&self, channel: Id) -> Result<VoiceStatePage, ClientError> {
        self.get(&with_id(r::CHANNEL_VOICE_STATES, channel)).await
    }

    // ---- S50 key routes

    /// K16 `POST /api/v1/me/voice/keys/key_packages`.
    pub async fn upload_key_packages(
        &self,
        req: &KeyPackageUploadRequest,
    ) -> Result<KeyPackageUploadResponse, ClientError> {
        self.post(r::ME_VOICE_KEY_PACKAGES, req).await
    }

    /// K2 `POST /api/v1/me/voice/keys/join` (after a move).
    pub async fn key_join(&self, req: &KeyJoinRequest) -> Result<CallKeysInit, ClientError> {
        self.post(r::ME_VOICE_KEYS_JOIN, req).await
    }

    /// K3 `POST /api/v1/calls/{call_id}/keys/state`.
    pub async fn call_keys_state(
        &self,
        call: &CallId,
        req: &KeyStateRequest,
    ) -> Result<KeyStateResponse, ClientError> {
        self.post(&with_call_id(r::CALL_KEYS_STATE, call.as_str()), req)
            .await
    }

    /// K4 `POST /api/v1/calls/{call_id}/keys/commit`.
    pub async fn call_keys_commit(
        &self,
        call: &CallId,
        req: &KeyCommitRequest,
    ) -> Result<KeyCommitAccepted, ClientError> {
        self.post(&with_call_id(r::CALL_KEYS_COMMIT, call.as_str()), req)
            .await
    }

    /// K5 `POST /api/v1/calls/{call_id}/keys/ready`.
    pub async fn call_keys_ready(
        &self,
        call: &CallId,
        req: &KeyReadyRequest,
    ) -> Result<Value, ClientError> {
        self.post(&with_call_id(r::CALL_KEYS_READY, call.as_str()), req)
            .await
    }

    /// K6 `POST /api/v1/calls/{call_id}/keys/invalid` (a re-add).
    pub async fn call_keys_invalid(
        &self,
        call: &CallId,
        req: &KeyInvalidRequest,
    ) -> Result<Value, ClientError> {
        self.post(&with_call_id(r::CALL_KEYS_INVALID, call.as_str()), req)
            .await
    }

    /// K7 `POST /api/v1/calls/{call_id}/keys/key_package` (a group reset).
    pub async fn call_keys_reset_package(
        &self,
        call: &CallId,
        req: &KeyResetPackageRequest,
    ) -> Result<Value, ClientError> {
        self.post(&with_call_id(r::CALL_KEYS_KEY_PACKAGE, call.as_str()), req)
            .await
    }

    /// K8 `POST /api/v1/calls/{call_id}/keys/release` (the recorder key handoff).
    pub async fn call_keys_release(
        &self,
        call: &CallId,
        req: &KeyReleaseRequest,
    ) -> Result<Value, ClientError> {
        self.post(&with_call_id(r::CALL_KEYS_RELEASE, call.as_str()), req)
            .await
    }

    /// K9 `POST /api/v1/calls/{call_id}/keys/rekey`.
    pub async fn call_keys_rekey(
        &self,
        call: &CallId,
        req: &KeyRekeyRequest,
    ) -> Result<Value, ClientError> {
        self.post(&with_call_id(r::CALL_KEYS_REKEY, call.as_str()), req)
            .await
    }

    /// K10 `POST /api/v1/calls/{call_id}/keys/report`.
    pub async fn call_keys_report(
        &self,
        call: &CallId,
        req: &KeyReportRequest,
    ) -> Result<Value, ClientError> {
        self.post(&with_call_id(r::CALL_KEYS_REPORT, call.as_str()), req)
            .await
    }

    // ---- S50 device routes

    /// K11 `POST /api/v1/devices` (enrol).
    pub async fn device_enroll(
        &self,
        req: &DeviceEnrollRequest,
    ) -> Result<DeviceCertificateResponse, ClientError> {
        self.post(r::DEVICES, req).await
    }

    /// K12 `POST /api/v1/devices/{id}/renew`.
    pub async fn device_renew(&self, device: Id) -> Result<DeviceCertificateResponse, ClientError> {
        self.post(&with_id(r::DEVICE_RENEW, device), &EmptyRequest {})
            .await
    }

    /// K13 `GET /api/v1/me/devices`.
    pub async fn my_devices(&self) -> Result<DevicePage, ClientError> {
        self.get(r::ME_DEVICES).await
    }

    /// K14 `POST /api/v1/devices/{id}/revoke`.
    pub async fn device_revoke(
        &self,
        device: Id,
        req: &DeviceRevokeRequest,
    ) -> Result<DeviceRevokeResponse, ClientError> {
        self.post(&with_id(r::DEVICE_REVOKE, device), req).await
    }

    /// K15 `GET /api/v1/devices/crl?after=&limit=`.
    pub async fn devices_crl(&self, q: &CrlQuery) -> Result<CrlPage, ClientError> {
        self.get(&with_query(
            r::DEVICES_CRL,
            &[
                ("after", q.after.map(|v| v.to_string())),
                ("limit", q.limit.map(|v| v.to_string())),
            ],
        ))
        .await
    }
}
