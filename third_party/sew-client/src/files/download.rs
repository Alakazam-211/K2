//! Signed-URL downloads (S25 §9.1): a plain `GET` with no `Authorization`, ranges, no redirects.

use std::fmt;

use bytes::Bytes;
use futures_util::stream::{self, BoxStream, StreamExt as _};

use crate::api::Client;
use crate::error::{ClientError, from_response};
use crate::meta::header;
use crate::secret::redact_url;
use crate::transport::{Transport, tcp};

/// A byte range to request: `bytes=<start>-[<end>]` (`end` inclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ByteRange {
    /// First byte.
    pub start: u64,
    /// Last byte (inclusive); `None` = to the end.
    pub end: Option<u64>,
}

impl ByteRange {
    /// `start..end` (end exclusive) as an inclusive HTTP range.
    pub fn span(start: u64, end_exclusive: u64) -> ByteRange {
        ByteRange {
            start,
            end: Some(end_exclusive.saturating_sub(1)),
        }
    }

    /// From `start` to the end.
    pub fn from(start: u64) -> ByteRange {
        ByteRange { start, end: None }
    }

    fn header(&self) -> String {
        match self.end {
            Some(e) => format!("bytes={}-{e}", self.start),
            None => format!("bytes={}-", self.start),
        }
    }
}

/// A `Content-Range: bytes <start>-<end>/<total>` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentRange {
    /// First byte.
    pub start: u64,
    /// Last byte (inclusive).
    pub end: u64,
    /// The whole size, when known.
    pub total: Option<u64>,
}

impl ContentRange {
    /// Parses `bytes a-b/total` (`total` may be `*`).
    pub fn parse(v: &str) -> Option<ContentRange> {
        let rest = v.trim().strip_prefix("bytes ")?;
        let (range, total) = rest.split_once('/')?;
        let (a, b) = range.split_once('-')?;
        Some(ContentRange {
            start: a.trim().parse().ok()?,
            end: b.trim().parse().ok()?,
            total: match total.trim() {
                "*" => None,
                t => Some(t.parse().ok()?),
            },
        })
    }
}

/// A download in progress: status, headers of interest, and the body as a stream.
pub struct Download {
    /// 200 or 206.
    pub status: u16,
    /// `Content-Type`.
    pub content_type: Option<String>,
    /// `Content-Length`.
    pub content_length: Option<u64>,
    /// `Content-Range` on a 206.
    pub content_range: Option<ContentRange>,
    /// The body chunks.
    pub body: BoxStream<'static, Result<Bytes, ClientError>>,
}

impl fmt::Debug for Download {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Download")
            .field("status", &self.status)
            .field("content_type", &self.content_type)
            .field("content_length", &self.content_length)
            .field("content_range", &self.content_range)
            .finish_non_exhaustive()
    }
}

impl Download {
    /// Reads the whole body.
    pub async fn bytes(mut self) -> Result<Vec<u8>, ClientError> {
        let mut out = Vec::new();
        while let Some(chunk) = self.body.next().await {
            out.extend_from_slice(&chunk?);
        }
        Ok(out)
    }
}

impl Client {
    /// `GET` a signed URL (S25 §9.1): no `Authorization` header, even on the base host; no
    /// redirects (a 3xx is `UnexpectedStatus`). A relative URL (starting with `/`) is joined to
    /// the base URL. Accepts 200 and 206; 416 maps to `Api` (`range_not_satisfiable`). Not
    /// retried (the caller resumes with a range).
    pub async fn download(
        &self,
        url: &str,
        range: Option<ByteRange>,
    ) -> Result<Download, ClientError> {
        let (base, http) = match &self.inner.transport {
            Transport::Tcp { base, http } => (base, http),
            #[cfg(all(feature = "mgmt", unix))]
            Transport::Uds { .. } => {
                return Err(ClientError::Unsupported(
                    "downloads are not available over the mgmt socket in S25.1",
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
        // A fresh request with no default headers: never the Bearer.
        let mut rb = http
            .get(full)
            .header("user-agent", self.inner.user_agent.as_str());
        if let Some(r) = range {
            rb = rb.header("range", r.header());
        }
        // The request timeout bounds the wait for the headers only; a large body streams for as
        // long as it takes (the caller bounds it if it wants to).
        let resp = match tokio::time::timeout(self.inner.request_timeout, rb.send()).await {
            Ok(r) => r.map_err(tcp::map_error)?,
            Err(_) => {
                return Err(ClientError::Transport(crate::error::TransportError::new(
                    crate::error::TransportErrorKind::Timeout,
                    true,
                    format!(
                        "no response headers within {:?}",
                        self.inner.request_timeout
                    ),
                )));
            }
        };
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
        if status != 200 && status != 206 {
            let body = resp.bytes().await.map_err(tcp::map_error)?;
            return Err(from_response(status, &headers, &body));
        }
        let body = stream::unfold(Some(resp), |resp| async move {
            let mut resp = resp?;
            match resp.chunk().await {
                Ok(Some(b)) => Some((Ok(b), Some(resp))),
                Ok(None) => None,
                Err(e) => Some((Err(ClientError::Transport(tcp::map_error(e))), None)),
            }
        })
        .boxed();
        Ok(Download {
            status,
            content_type: header(&headers, "content-type").map(str::to_owned),
            content_length: header(&headers, "content-length").and_then(|v| v.parse().ok()),
            content_range: header(&headers, "content-range").and_then(ContentRange::parse),
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(ByteRange::span(100, 200).header(), "bytes=100-199");
        assert_eq!(ByteRange::from(7).header(), "bytes=7-");
        assert_eq!(
            ContentRange::parse("bytes 100-199/5000"),
            Some(ContentRange {
                start: 100,
                end: 199,
                total: Some(5000)
            })
        );
        assert_eq!(
            ContentRange::parse("bytes 0-1/*").and_then(|c| c.total),
            None
        );
        assert_eq!(ContentRange::parse("items 0-1/2"), None);
    }
}
