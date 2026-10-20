//! S35 v2 recording calls (prd-sew-35 v2 §19.1): start, stop, the in-room consent answer, the
//! audio keep days, and the reads. Agents get `403 agents_cannot_record` on the writes and
//! `403 agents_no_audio` on the audio listing (§17.1).

use sew_proto::http::{EmptyRequest, Page};
use sew_proto::id::Id;
use sew_proto::recording::{
    ActiveRecording, AudioRetentionRequest, ConsentRequest, Recording, StartRecordingRequest,
};
use sew_proto::routes::{self as r, with_id};

use crate::{Client, ClientError};

impl Client {
    /// `POST /api/v1/channels/{id}/recording/start`: starts recording the channel's (or DM's)
    /// live call. The caller needs RECORD_CALLS and a connected voice entry in the room; the
    /// template shows the consent modal as a confirmation first (§9.10).
    pub async fn recording_start(
        &self,
        channel: Id,
        req: &StartRecordingRequest,
    ) -> Result<Recording, ClientError> {
        self.post(&with_id(r::CHANNEL_RECORDING_START, channel), req)
            .await
    }

    /// `POST /api/v1/recordings/{id}/stop`.
    pub async fn recording_stop(&self, recording: Id) -> Result<Recording, ClientError> {
        self.post(&with_id(r::RECORDING_STOP, recording), &EmptyRequest {})
            .await
    }

    /// `POST /api/v1/recordings/{id}/consent`: the caller's answer (grant, decline or
    /// withdraw), echoing the prompt's `notice_version` (rev 91).
    pub async fn recording_consent(
        &self,
        recording: Id,
        consent: bool,
        notice_version: Option<u32>,
    ) -> Result<Recording, ClientError> {
        let req = ConsentRequest {
            consent,
            notice_version,
        };
        self.post(&with_id(r::RECORDING_CONSENT, recording), &req)
            .await
    }

    /// `GET /api/v1/recordings/{id}` (masked `404` unless the caller can read the record
    /// message).
    pub async fn recording_get(&self, recording: Id) -> Result<Recording, ClientError> {
        self.get(&with_id(r::RECORDING, recording)).await
    }

    /// `GET /api/v1/channels/{id}/recordings?before=&limit=`: the channel's recordings, newest
    /// first.
    pub async fn channel_recordings(
        &self,
        channel: Id,
        before: Option<Id>,
        limit: Option<u32>,
    ) -> Result<Page<Recording>, ClientError> {
        let mut q: Vec<String> = Vec::new();
        if let Some(b) = before {
            q.push(format!("before={b}"));
        }
        if let Some(l) = limit {
            q.push(format!("limit={l}"));
        }
        let mut path = with_id(r::CHANNEL_RECORDINGS, channel);
        if !q.is_empty() {
            path = format!("{path}?{}", q.join("&"));
        }
        self.get(&path).await
    }

    /// `GET /api/v1/channels/{id}/recording`: the channel's active recording, if any.
    pub async fn channel_recording(&self, channel: Id) -> Result<ActiveRecording, ClientError> {
        self.get(&with_id(r::CHANNEL_RECORDING, channel)).await
    }

    /// `POST /api/v1/recordings/{id}/audio/retention`: new audio keep days (the starter and
    /// MANAGE_CHANNELS holders; clamped; shorter takes effect at once).
    pub async fn set_audio_retention(
        &self,
        recording: Id,
        audio_keep_days: u16,
    ) -> Result<Recording, ClientError> {
        let req = AudioRetentionRequest { audio_keep_days };
        self.post(&with_id(r::RECORDING_AUDIO_RETENTION, recording), &req)
            .await
    }
}
