//! Response metadata (S25 §6.1, §8.3) and the raw response (S8).

use std::time::{Duration, SystemTime};

use serde::de::DeserializeOwned;
use sew_proto::error::ErrorBody;
use sew_proto::http::IdempotencyKey;

use crate::error::{ClientError, snippet};

/// The first value of `name` (case-insensitive) in a header list.
pub fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// A typed value and what came with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Response<T> {
    /// The parsed body.
    pub value: T,
    /// Status, headers of interest, retry count.
    pub meta: ResponseMeta,
}

/// What a response said besides its body.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResponseMeta {
    /// HTTP status.
    pub status: u16,
    /// `X-Request-Id`.
    pub request_id: Option<String>,
    /// `X-RateLimit-*`, when all three are present and numeric.
    pub rate_limit: Option<RateLimitInfo>,
    /// `Idempotent-Replayed: true`: the server answered from its store.
    pub replayed: bool,
    /// The idempotency key that was sent.
    pub idempotency_key: Option<IdempotencyKey>,
    /// Attempts made (1 = no retry).
    pub attempts: u32,
    /// The `Date` header.
    pub server_date: Option<SystemTime>,
}

impl ResponseMeta {
    pub(crate) fn from_headers(
        status: u16,
        headers: &[(String, String)],
        key: Option<IdempotencyKey>,
        attempts: u32,
    ) -> ResponseMeta {
        ResponseMeta {
            status,
            request_id: header(headers, "x-request-id").map(str::to_owned),
            rate_limit: RateLimitInfo::from_headers(headers),
            replayed: header(headers, "idempotent-replayed")
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("true")),
            idempotency_key: key,
            attempts,
            server_date: header(headers, "date").and_then(parse_http_date),
        }
    }
}

/// `X-RateLimit-Limit`, `-Remaining`, `-Reset-Ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RateLimitInfo {
    /// Bucket size.
    pub limit: u32,
    /// Requests left.
    pub remaining: u32,
    /// Milliseconds until the bucket refills.
    pub reset_ms: u64,
}

impl RateLimitInfo {
    /// All three headers present and numeric, else `None` (never an error).
    pub fn from_headers(headers: &[(String, String)]) -> Option<RateLimitInfo> {
        Some(RateLimitInfo {
            limit: header(headers, "x-ratelimit-limit")?.trim().parse().ok()?,
            remaining: header(headers, "x-ratelimit-remaining")?
                .trim()
                .parse()
                .ok()?,
            reset_ms: header(headers, "x-ratelimit-reset-ms")?
                .trim()
                .parse()
                .ok()?,
        })
    }
}

/// An HTTP `Date` (RFC 7231 IMF-fixdate, which RFC 2822 parsing accepts).
pub(crate) fn parse_http_date(v: &str) -> Option<SystemTime> {
    let dt = chrono::DateTime::parse_from_rfc2822(v.trim()).ok()?;
    let ms = u64::try_from(dt.timestamp_millis()).ok()?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_millis(ms))
}

/// A raw response: status, headers and bytes (S8's convention-test shape).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawResponse {
    /// HTTP status.
    pub status: u16,
    /// Headers, names lowercase, in order.
    pub headers: Vec<(String, String)>,
    /// Body bytes.
    pub body: Vec<u8>,
}

impl RawResponse {
    /// The first value of `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        header(&self.headers, name)
    }

    /// The body as JSON.
    pub fn json<T: DeserializeOwned>(&self) -> Result<T, ClientError> {
        serde_json::from_slice(&self.body).map_err(|e| ClientError::Decode {
            status: self.status,
            body_snippet: snippet(&self.body),
            error: e.to_string(),
        })
    }

    /// The body as text (lossy).
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The error envelope, if the body is one (S1's type).
    pub fn error_body(&self) -> Option<ErrorBody> {
        serde_json::from_slice(&self.body).ok()
    }

    /// `Ok(self)` for a 2xx, else the typed error.
    pub fn error_for_status(self) -> Result<RawResponse, ClientError> {
        if (200..300).contains(&self.status) {
            Ok(self)
        } else {
            Err(crate::error::from_response(
                self.status,
                &self.headers,
                &self.body,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_date_parses() {
        let t = parse_http_date("Sun, 27 Sep 2026 12:00:00 GMT");
        let want = SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_510_400);
        assert_eq!(t, Some(want));
        assert_eq!(parse_http_date("yesterday"), None);
    }
}
