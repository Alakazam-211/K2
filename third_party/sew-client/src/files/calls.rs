//! S15 calls (prd-sew-15 §6): the raw typed upload steps, file lists and attachment removal.
//! The resumable helpers (`upload_file`, `download_attachment_to`) come with S25.3.
//!
//! Signed URLs (`upload_url` with `UPLOAD_DATA`; attachment URLs on `FILE`, `FILE_THUMB`,
//! `FILE_PREVIEW`, `FILE_POSTER`) are fetched without `Authorization`; the byte routes go
//! through [`Client::download`].

use bytes::Bytes;
use sew_proto::api::files::{
    ChannelFile, CreateUploadRequest, CreateUploadResponse, FilesQuery, RemoveAttachmentsRequest,
    Upload, UploadStatus,
};
use sew_proto::api::messages::Message;
use sew_proto::http::{EmptyRequest, OkResponse, Page};
use sew_proto::id::Id;
use sew_proto::routes::{self as r, with_id};

use crate::api::{Client, decode};
use crate::error::{ClientError, from_response};
use crate::secret::redact_url;
use crate::transport::{Transport, tcp};

/// One `Content-Range` of an upload: bytes `start..=end` of `total`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UploadRange {
    /// First byte.
    pub start: u64,
    /// Last byte (inclusive).
    pub end: u64,
    /// The declared size.
    pub total: u64,
}

impl UploadRange {
    fn header(&self) -> String {
        format!("bytes {}-{}/{}", self.start, self.end, self.total)
    }
}

impl Client {
    /// `POST /api/v1/channels/{id}/uploads` (`CHANNEL_UPLOADS`): step 1 of an upload.
    pub async fn channel_uploads_create(
        &self,
        channel: Id,
        req: &CreateUploadRequest,
    ) -> Result<CreateUploadResponse, ClientError> {
        self.post(&with_id(r::CHANNEL_UPLOADS, channel), req).await
    }

    /// `GET /api/v1/uploads/{id}` (`UPLOAD`): the caller's upload with a fresh `upload_url`.
    pub async fn upload_get(&self, id: Id) -> Result<Upload, ClientError> {
        self.get(&with_id(r::UPLOAD, id)).await
    }

    /// `POST /api/v1/uploads/{id}/abort` (`UPLOAD_ABORT`).
    pub async fn upload_abort(&self, id: Id) -> Result<OkResponse, ClientError> {
        self.post(&with_id(r::UPLOAD_ABORT, id), &EmptyRequest::default())
            .await
    }

    /// `GET /api/v1/channels/{id}/files` (`CHANNEL_FILES`).
    pub async fn channel_files(
        &self,
        channel: Id,
        q: &FilesQuery,
    ) -> Result<Page<ChannelFile>, ClientError> {
        let qs = serde_urlencoded::to_string(q)
            .map_err(|e| ClientError::invalid("query", e.to_string()))?;
        let path = with_id(r::CHANNEL_FILES, channel);
        if qs.is_empty() {
            self.get(&path).await
        } else {
            self.get(&format!("{path}?{qs}")).await
        }
    }

    /// `GET /api/v1/files/{id}` (`FILE`) with Bearer: one file with fresh URLs.
    pub async fn file_get(&self, id: Id) -> Result<ChannelFile, ClientError> {
        self.get(&with_id(r::FILE, id)).await
    }

    /// `POST /api/v1/messages/{id}/attachments/remove` (`MESSAGE_ATTACHMENTS_REMOVE`).
    pub async fn message_attachments_remove(
        &self,
        message: Id,
        req: &RemoveAttachmentsRequest,
    ) -> Result<Message, ClientError> {
        self.post(&with_id(r::MESSAGE_ATTACHMENTS_REMOVE, message), req)
            .await
    }

    /// `POST <upload_url>` (`UPLOAD_DATA`, signed): one range of bytes, no `Authorization`.
    /// Not retried (the caller resumes from `committed_bytes`).
    pub async fn upload_data(
        &self,
        upload_url: &str,
        range: UploadRange,
        bytes: Bytes,
    ) -> Result<UploadStatus, ClientError> {
        self.signed(http::Method::POST, upload_url, Some((range, bytes)))
            .await
    }

    /// `GET <upload_url>` (`UPLOAD_DATA`, signed): the committed offset after a cut.
    pub async fn upload_data_status(&self, upload_url: &str) -> Result<UploadStatus, ClientError> {
        self.signed(http::Method::GET, upload_url, None).await
    }

    async fn signed(
        &self,
        method: http::Method,
        url: &str,
        body: Option<(UploadRange, Bytes)>,
    ) -> Result<UploadStatus, ClientError> {
        let (base, http) = match &self.inner.transport {
            Transport::Tcp { base, http } => (base, http),
            #[cfg(all(feature = "mgmt", unix))]
            Transport::Uds { .. } => {
                return Err(ClientError::Unsupported(
                    "signed upload URLs are fetched over TCP, not the mgmt socket",
                ));
            }
        };
        let full = if url.starts_with('/') {
            format!("{base}{url}")
        } else if url.starts_with("http://") || url.starts_with("https://") {
            url.to_owned()
        } else {
            return Err(ClientError::invalid(
                "url",
                format!("{} is neither absolute nor /-relative", redact_url(url)),
            ));
        };
        let mut rb = http
            .request(method, full)
            .header("user-agent", self.inner.user_agent.as_str());
        if let Some((range, bytes)) = body {
            rb = rb
                .header("content-range", range.header())
                .header("content-type", "application/octet-stream")
                .body(bytes);
        }
        let resp = rb.send().await.map_err(tcp::map_error)?;
        let status = resp.status().as_u16();
        let headers: Vec<(String, String)> = resp
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(v.as_bytes()).into_owned(),
                )
            })
            .collect();
        let body = resp.bytes().await.map_err(tcp::map_error)?;
        if !(200..300).contains(&status) {
            return Err(from_response(status, &headers, &body));
        }
        decode(status, &body)
    }
}
