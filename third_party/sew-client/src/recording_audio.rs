//! S35 v2 recording audio calls (prd-sew-35 v2 §12.1, §19.2, §19.4): keep days, the audio
//! listing, and the compliance download. Download answers carry per-item keys: callers keep
//! them out of logs and write them only where the user asked (`sewc recordings download`).

use serde::Serialize;
use sew_proto::http::Page;
use sew_proto::id::Id;
use sew_proto::recording::{
    AudioRetentionRequest, AudioSegment, DownloadRequest, DownloadSegment, Recording,
    RecordingAudio, RecordingDownload,
};
use sew_proto::routes as r;

use crate::api::with_query;
use crate::{Client, ClientError};

#[derive(Serialize)]
struct After {
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<Id>,
}

fn rec_path(template: &str, rec: Id) -> String {
    template.replace("{id}", &rec.to_string())
}

impl Client {
    /// `POST /api/v1/recordings/{id}/audio/retention`.
    pub async fn recording_audio_retention(
        &self,
        rec: Id,
        audio_keep_days: u16,
    ) -> Result<Recording, ClientError> {
        self.post(
            &rec_path(r::RECORDING_AUDIO_RETENTION, rec),
            &AudioRetentionRequest { audio_keep_days },
        )
        .await
    }

    /// `GET /api/v1/recordings/{id}/audio`.
    pub async fn recording_audio(&self, rec: Id) -> Result<RecordingAudio, ClientError> {
        self.get(&rec_path(r::RECORDING_AUDIO, rec)).await
    }

    /// `GET /api/v1/recordings/{id}/audio/tracks/{member_id}?after=`.
    pub async fn recording_audio_track(
        &self,
        rec: Id,
        member: Id,
        after: Option<Id>,
    ) -> Result<Page<AudioSegment>, ClientError> {
        let p = rec_path(r::RECORDING_AUDIO_TRACK, rec).replace("{member_id}", &member.to_string());
        self.get(&with_query(&p, Some(&After { after }))?).await
    }

    /// `POST /api/v1/compliance/recordings/{id}/downloads` (compliance-flag holders).
    pub async fn recording_download_create(
        &self,
        rec: Id,
        req: &DownloadRequest,
    ) -> Result<RecordingDownload, ClientError> {
        self.post(&rec_path(r::COMPLIANCE_RECORDING_DOWNLOADS, rec), req)
            .await
    }

    /// `GET /api/v1/compliance/recordings/{id}/downloads/{download_id}/segments?after=`.
    pub async fn recording_download_segments(
        &self,
        rec: Id,
        download: Id,
        after: Option<Id>,
    ) -> Result<Page<DownloadSegment>, ClientError> {
        let p = rec_path(r::COMPLIANCE_RECORDING_DOWNLOAD_SEGMENTS, rec)
            .replace("{download_id}", &download.to_string());
        self.get(&with_query(&p, Some(&After { after }))?).await
    }

    /// One download URL (`/api/v1/compliance/recordings/{id}/files/{link_id}?d=&exp=&sig=`,
    /// as a grant listed it): the recorder-sealed bytes. The URL is viewer-bound, so the
    /// request carries this client's Bearer.
    pub async fn recording_download_file(&self, url: &str) -> Result<Vec<u8>, ClientError> {
        if !url.starts_with("/api/v1/compliance/recordings/") || !url.contains("/files/") {
            return Err(ClientError::invalid(
                "url",
                "a download URL starts with /api/v1/compliance/recordings/",
            ));
        }
        let resp = self.request("GET", url, &[], None).await?;
        if resp.status != 200 && resp.status != 206 {
            return Err(crate::error::from_response(
                resp.status,
                &resp.headers,
                &resp.body,
            ));
        }
        Ok(resp.body)
    }
}
