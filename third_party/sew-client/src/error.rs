//! Typed errors (S25 §7). Mapping is by the C5 `code`, never by status alone.

use std::fmt;
use std::time::Duration;

use serde_json::{Map, Value};
use sew_proto::error::{ApiError, Code, ErrorBody};

use crate::meta::{RateLimitInfo, header};
use crate::secret::scrub;

/// Longest body snippet kept in an error.
pub const SNIPPET_MAX: usize = 256;

/// Every failure a client call can return.
#[derive(Debug)]
#[non_exhaustive]
pub enum ClientError {
    /// The server answered with a C5 error envelope.
    Api(Box<ApiFailure>),
    /// A non-2xx answer that is not an envelope (proxy HTML, an empty 502, a redirect).
    UnexpectedStatus {
        /// HTTP status.
        status: u16,
        /// The first bytes of the body (at most 256), lossy UTF-8.
        body_snippet: String,
    },
    /// The request did not complete: connect, timeout, reset, TLS, UDS.
    Transport(TransportError),
    /// A 2xx body that is not the expected type.
    Decode {
        /// HTTP status.
        status: u16,
        /// The first bytes of the body.
        body_snippet: String,
        /// What failed.
        error: String,
    },
    /// A client-side check failed; nothing was sent.
    InvalidInput {
        /// The field.
        field: String,
        /// Why.
        hint: String,
    },
    /// Not available on this client (for example the gateway when acting as an agent).
    Unsupported(&'static str),
    /// A gateway failure.
    Gateway(GatewayError),
    /// A bad base URL, token file mode, missing profile, bad state file.
    Config(String),
}

/// A C5 error envelope and the response around it.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiFailure {
    /// HTTP status.
    pub status: u16,
    /// The code; [`Code::Unknown`] for a code this build does not know.
    pub code: Code,
    /// The raw code string, always kept.
    pub code_str: String,
    /// For humans.
    pub hint: String,
    /// Code-specific details.
    pub details: Map<String, Value>,
    /// `X-Request-Id`.
    pub request_id: Option<String>,
    /// `X-RateLimit-*`.
    pub rate_limit: Option<RateLimitInfo>,
    /// `details.retry_after_ms`, else `Retry-After` seconds.
    pub retry_after: Option<Duration>,
    /// Response headers, names lowercase, in order.
    pub headers: Vec<(String, String)>,
}

/// What a `410 resync_required` says (C8, contracts revs 16, 29).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResyncInfo {
    /// The change-log floor.
    pub floor: u64,
    /// The log head, when sent.
    pub head: Option<u64>,
    /// `details.reason` (`behind_floor`, `ahead_of_head`), when sent.
    pub reason: Option<String>,
    /// The install's `sync_epoch` (opaque), when sent.
    pub sync_epoch: Option<String>,
}

impl ApiFailure {
    /// Parses an error response. `None` when the body is not a C5 envelope.
    pub fn parse(status: u16, headers: &[(String, String)], body: &[u8]) -> Option<ApiFailure> {
        let v: Value = serde_json::from_slice(body).ok()?;
        let obj = v.as_object()?;
        if obj.get("ok") != Some(&Value::Bool(false)) {
            return None;
        }
        let err = obj.get("error")?.as_object()?;
        let code_str = err.get("code")?.as_str()?.to_owned();
        let hint = err
            .get("hint")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let details = match err.get("details") {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        let retry_after = details
            .get("retry_after_ms")
            .and_then(Value::as_u64)
            .map(Duration::from_millis)
            .or_else(|| {
                header(headers, "retry-after")
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .map(Duration::from_secs)
            });
        Some(ApiFailure {
            status,
            code: Code::from_wire(&code_str).unwrap_or(Code::Unknown),
            code_str,
            hint,
            details,
            request_id: header(headers, "x-request-id").map(str::to_owned),
            rate_limit: RateLimitInfo::from_headers(headers),
            retry_after,
            headers: headers.to_vec(),
        })
    }

    /// `details.reason` (contracts revs 6, 8).
    pub fn reason(&self) -> Option<&str> {
        self.details.get("reason").and_then(Value::as_str)
    }

    /// `details.field`.
    pub fn validation_field(&self) -> Option<&str> {
        self.details.get("field").and_then(Value::as_str)
    }

    /// For a 410: `floor`, `head`, `reason`, `sync_epoch`.
    pub fn resync(&self) -> Option<ResyncInfo> {
        if self.code != Code::ResyncRequired {
            return None;
        }
        Some(ResyncInfo {
            floor: self.details.get("floor").and_then(Value::as_u64)?,
            head: self.details.get("head").and_then(Value::as_u64),
            reason: self.reason().map(str::to_owned),
            sync_epoch: self.details.get("sync_epoch").and_then(epoch_string),
        })
    }

    /// The first value of a response header (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        header(&self.headers, name)
    }

    /// The envelope as S1's type (`code` is `Unknown` for codes this build does not know).
    pub fn body(&self) -> ErrorBody {
        ErrorBody::new(ApiError {
            code: self.code,
            hint: self.hint.clone(),
            details: self.details.clone(),
        })
    }
}

/// A `sync_epoch` value: a decimal string on the wire (rev 8); a number is accepted too.
pub(crate) fn epoch_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

impl fmt::Display for ApiFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}): {}", self.code_str, self.status, self.hint)?;
        if let Some(r) = &self.request_id {
            let short: String = r.chars().take(8).collect();
            write!(f, " [req {short}…]")?;
        }
        Ok(())
    }
}

/// How a transport failure happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TransportErrorKind {
    /// Connecting failed (refused, DNS, UDS connect).
    Connect,
    /// The request or the connect timed out.
    Timeout,
    /// The connection was reset or closed mid-exchange.
    Reset,
    /// TLS failed.
    Tls,
    /// Another IO error.
    Io,
    /// The peer broke HTTP.
    Protocol,
}

/// A request that did not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError {
    /// How it failed.
    pub kind: TransportErrorKind,
    /// `false` when the request provably never left (connect failed).
    pub sent: bool,
    /// What happened (secrets cut).
    pub message: String,
}

impl TransportError {
    /// A transport error (the message is scrubbed of secrets).
    pub fn new(kind: TransportErrorKind, sent: bool, message: impl Into<String>) -> Self {
        TransportError {
            kind,
            sent,
            message: scrub(&message.into()),
        }
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} ({}): {}",
            self.kind,
            if self.sent { "sent" } else { "not sent" },
            self.message
        )
    }
}

/// A gateway failure returned by [`crate::gateway::Gateway`] methods.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum GatewayError {
    /// The gateway stopped (after `Stopped` or `close`); make a new one.
    Closed,
    /// A local limit check failed; nothing was sent.
    InvalidInput {
        /// The field.
        field: String,
        /// Why.
        hint: String,
    },
}

impl fmt::Display for GatewayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GatewayError::Closed => f.write_str("the gateway is closed"),
            GatewayError::InvalidInput { field, hint } => write!(f, "{field}: {hint}"),
        }
    }
}

impl std::error::Error for GatewayError {}

impl ClientError {
    /// The C5 code, when the server sent an envelope.
    pub fn code(&self) -> Option<Code> {
        match self {
            ClientError::Api(a) => Some(a.code),
            _ => None,
        }
    }

    /// The HTTP status, when the server answered.
    pub fn status(&self) -> Option<u16> {
        match self {
            ClientError::Api(a) => Some(a.status),
            ClientError::UnexpectedStatus { status, .. } | ClientError::Decode { status, .. } => {
                Some(*status)
            }
            _ => None,
        }
    }

    /// The envelope, when the server sent one.
    pub fn api(&self) -> Option<&ApiFailure> {
        match self {
            ClientError::Api(a) => Some(a),
            _ => None,
        }
    }

    /// 404 `not_found` (masked or missing: the same thing).
    pub fn is_not_found(&self) -> bool {
        self.code() == Some(Code::NotFound)
    }

    /// 401 `unauthenticated` or `session_expired`.
    pub fn is_auth(&self) -> bool {
        matches!(
            self.code(),
            Some(Code::Unauthenticated | Code::SessionExpired)
        )
    }

    /// 429 `rate_limited`.
    pub fn is_rate_limited(&self) -> bool {
        self.code() == Some(Code::RateLimited)
    }

    /// 410 `resync_required`.
    pub fn is_resync_required(&self) -> bool {
        self.code() == Some(Code::ResyncRequired)
    }

    /// The server's wait hint.
    pub fn retry_after(&self) -> Option<Duration> {
        self.api().and_then(|a| a.retry_after)
    }

    pub(crate) fn invalid(field: &str, hint: impl Into<String>) -> ClientError {
        ClientError::InvalidInput {
            field: field.to_owned(),
            hint: hint.into(),
        }
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Api(a) => fmt::Display::fmt(a, f),
            ClientError::UnexpectedStatus {
                status,
                body_snippet,
            } => write!(f, "unexpected HTTP {status}: {}", scrub(body_snippet)),
            ClientError::Transport(t) => write!(f, "transport: {t}"),
            ClientError::Decode {
                status,
                body_snippet,
                error,
            } => write!(
                f,
                "HTTP {status}: bad response body ({error}): {}",
                scrub(body_snippet)
            ),
            ClientError::InvalidInput { field, hint } => write!(f, "invalid {field}: {hint}"),
            ClientError::Unsupported(what) => write!(f, "unsupported: {what}"),
            ClientError::Gateway(g) => write!(f, "gateway: {g}"),
            ClientError::Config(m) => write!(f, "config: {m}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<TransportError> for ClientError {
    fn from(e: TransportError) -> Self {
        ClientError::Transport(e)
    }
}

impl From<GatewayError> for ClientError {
    fn from(e: GatewayError) -> Self {
        ClientError::Gateway(e)
    }
}

/// The first `SNIPPET_MAX` bytes of a body, lossy UTF-8, cut on a char boundary.
pub(crate) fn snippet(body: &[u8]) -> String {
    let cut = &body[..body.len().min(SNIPPET_MAX)];
    let mut s = String::from_utf8_lossy(cut).into_owned();
    while s.len() > SNIPPET_MAX {
        s.pop();
    }
    s
}

/// Maps a non-2xx response: an envelope becomes `Api`, anything else `UnexpectedStatus`.
pub(crate) fn from_response(status: u16, headers: &[(String, String)], body: &[u8]) -> ClientError {
    match ApiFailure::parse(status, headers, body) {
        Some(a) => ClientError::Api(Box::new(a)),
        None => ClientError::UnexpectedStatus {
            status,
            body_snippet: snippet(body),
        },
    }
}
