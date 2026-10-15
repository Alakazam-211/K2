//! Thin Sew mgmt client: HTTP/1.1 over Sew's mgmt Unix socket (S27 §9.2).
//!
//! K2 house style (hand-rolled HTTP like `cell_server.rs`), no new HTTP
//! crate, never a linked Sew client (G10: `sew-client` is not used; the
//! bodies come from [`k2_core::tool_hook::sew_wire`], the TEMP mirror of
//! `sew-proto` that K1 swaps for the vendored crate).
//!
//! - One request per connection: `Host: sew`, `Connection: close`.
//! - Every call: `Authorization: Bearer <token>`; every POST also
//!   `Idempotency-Key` and `Sew-Actor` (attribution only — KM13: Sew labels
//!   it "reported by K2"; it is never proof a person acted).
//! - Response read to EOF; body by `Content-Length`, de-chunked when
//!   `Transfer-Encoding: chunked`, else to EOF.
//! - Timeouts: connect 2 s, whole call 10 s.
//! - The token is read from the token file on every call, trimmed, never
//!   logged. An unreadable file is a transport-class failure.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use k2_core::tool_hook::sew_wire::{self, ErrorBody};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest response K2 reads (a 500-member page is well under this).
const MAX_RESPONSE: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct MgmtClient {
    pub socket: PathBuf,
    pub token_file: PathBuf,
}

/// A parsed response.
#[derive(Debug, Clone)]
pub struct MgmtResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl MgmtResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// Sew's error envelope, when the body is one.
    pub fn error(&self) -> Option<ErrorBody> {
        serde_json::from_slice(&self.body).ok()
    }

    /// `"<code>: <hint>"`, or `http <status>` + the first 200 bytes.
    pub fn error_line(&self) -> String {
        match self.error() {
            Some(e) => format!("{}: {}", e.error.code, e.error.hint),
            None => {
                let t = self.body_text();
                let head: String = t.chars().take(200).collect();
                format!("http {} {head}", self.status)
            }
        }
    }

    /// `Retry-After` (seconds) or `details.retry_after_ms`.
    pub fn retry_after(&self) -> Option<Duration> {
        if let Some(s) = self.header(sew_wire::RETRY_AFTER) {
            if let Ok(secs) = s.trim().parse::<u64>() {
                return Some(Duration::from_secs(secs));
            }
        }
        self.error()
            .and_then(|e| e.error.details.get("retry_after_ms").and_then(|v| v.as_u64()))
            .map(Duration::from_millis)
    }
}

/// Could not get an HTTP answer at all (socket missing, refused,
/// permission, timeout, EOF mid-head, token file unreadable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError(pub String);

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl MgmtClient {
    pub fn new(socket: impl Into<PathBuf>, token_file: impl Into<PathBuf>) -> Self {
        MgmtClient {
            socket: socket.into(),
            token_file: token_file.into(),
        }
    }

    fn token(&self) -> Result<String, TransportError> {
        read_token(&self.token_file)
    }

    pub async fn get(&self, path: &str) -> Result<MgmtResponse, TransportError> {
        self.call("GET", path, None, None, None).await
    }

    pub async fn post(
        &self,
        path: &str,
        body: &[u8],
        idem_key: &str,
        actor: &str,
    ) -> Result<MgmtResponse, TransportError> {
        self.call("POST", path, Some(body), Some(idem_key), Some(actor))
            .await
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
        idem_key: Option<&str>,
        actor: Option<&str>,
    ) -> Result<MgmtResponse, TransportError> {
        let token = self.token()?;
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: sew\r\nConnection: close\r\nAuthorization: Bearer {token}\r\nAccept: application/json\r\n"
        );
        if let Some(k) = idem_key {
            head.push_str(&format!("{}: {k}\r\n", sew_wire::IDEMPOTENCY_KEY));
        }
        if let Some(a) = actor {
            let a = k2_core::tool_hook::sew_actor_header(a);
            head.push_str(&format!("{}: {a}\r\n", sew_wire::SEW_ACTOR));
        }
        let body = body.unwrap_or(&[]);
        if method == "POST" {
            head.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                body.len()
            ));
        }
        head.push_str("\r\n");
        let fut = async {
            let mut stream = tokio::time::timeout(CONNECT_TIMEOUT, UnixStream::connect(&self.socket))
                .await
                .map_err(|_| TransportError(format!("connect {} timed out", self.socket.display())))?
                .map_err(|e| TransportError(format!("connect {}: {e}", self.socket.display())))?;
            let mut req = head.into_bytes();
            req.extend_from_slice(body);
            stream
                .write_all(&req)
                .await
                .map_err(|e| TransportError(format!("write: {e}")))?;
            let mut raw = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                match stream.read(&mut chunk).await {
                    Ok(0) => break,
                    Ok(n) => {
                        raw.extend_from_slice(&chunk[..n]);
                        if raw.len() > MAX_RESPONSE {
                            return Err(TransportError("response too large".into()));
                        }
                        if response_complete(&raw) {
                            break;
                        }
                    }
                    Err(e) => return Err(TransportError(format!("read: {e}"))),
                }
            }
            parse_response(&raw)
        };
        tokio::time::timeout(CALL_TIMEOUT, fut)
            .await
            .map_err(|_| TransportError("mgmt call timed out".into()))?
    }
}

/// Read and trim the token file. Never logs the token.
pub fn read_token(path: &Path) -> Result<String, TransportError> {
    let raw = std::fs::read_to_string(path).map_err(|_| {
        TransportError(format!("mgmt token file unreadable: {}", path.display()))
    })?;
    let t = raw.trim().to_string();
    if t.is_empty() {
        return Err(TransportError(format!(
            "mgmt token file unreadable: {} is empty",
            path.display()
        )));
    }
    Ok(t)
}

fn find_head_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|w| w == b"\r\n\r\n")
}

fn content_length(head: &str) -> Option<usize> {
    head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case("content-length")
            .then(|| v.trim().parse().ok())
            .flatten()
    })
}

fn is_chunked(head: &str) -> bool {
    head.lines().any(|l| {
        l.split_once(':').is_some_and(|(k, v)| {
            k.trim().eq_ignore_ascii_case("transfer-encoding")
                && v.to_ascii_lowercase().contains("chunked")
        })
    })
}

/// Stop reading early when the framing says the body is complete
/// (`Connection: close` servers also end with EOF).
fn response_complete(raw: &[u8]) -> bool {
    let Some(end) = find_head_end(raw) else {
        return false;
    };
    let head = String::from_utf8_lossy(&raw[..end]);
    let body = &raw[end + 4..];
    if is_chunked(&head) {
        return dechunk(body).is_some();
    }
    match content_length(&head) {
        Some(n) => body.len() >= n,
        None => false,
    }
}

/// Decode a complete chunked body; `None` while incomplete or malformed.
fn dechunk(mut body: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = body.windows(2).position(|w| w == b"\r\n")?;
        let size_line = std::str::from_utf8(&body[..line_end]).ok()?;
        let size_hex = size_line.split(';').next()?.trim();
        let size = usize::from_str_radix(size_hex, 16).ok()?;
        body = &body[line_end + 2..];
        if size == 0 {
            return Some(out);
        }
        if body.len() < size + 2 {
            return None;
        }
        out.extend_from_slice(&body[..size]);
        body = &body[size + 2..];
    }
}

pub fn parse_response(raw: &[u8]) -> Result<MgmtResponse, TransportError> {
    let end = find_head_end(raw).ok_or_else(|| {
        TransportError(if raw.is_empty() {
            "connection closed before a response (EOF)".into()
        } else {
            "truncated response head".into()
        })
    })?;
    let head = String::from_utf8_lossy(&raw[..end]).to_string();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| TransportError(format!("bad status line {status_line:?}")))?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    let rest = &raw[end + 4..];
    let body = if is_chunked(&head) {
        dechunk(rest).ok_or_else(|| TransportError("truncated chunked body".into()))?
    } else {
        match content_length(&head) {
            Some(n) if rest.len() < n => {
                return Err(TransportError("truncated body (EOF)".into()));
            }
            Some(n) => rest[..n].to_vec(),
            None => rest.to_vec(),
        }
    };
    Ok(MgmtResponse {
        status,
        headers,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_content_length_chunked_and_eof_bodies() {
        let r = parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"{}"[..]));
        let r = parse_response(
            b"HTTP/1.1 201 Created\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n{\"a\r\n4\r\n\":1}\r\n0\r\n\r\n",
        )
        .unwrap();
        assert_eq!(r.body_text(), "{\"a\":1}");
        let r = parse_response(b"HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\nnope").unwrap();
        assert_eq!((r.status, r.body_text().as_str()), (404, "nope"));
    }

    #[test]
    fn truncated_responses_are_transport_errors_not_successes() {
        assert!(parse_response(b"").is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n{}").is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nab").is_err());
    }

    #[test]
    fn retry_after_from_header_or_details() {
        let r = parse_response(b"HTTP/1.1 429 Too Many\r\nRetry-After: 7\r\nContent-Length: 0\r\n\r\n").unwrap();
        assert_eq!(r.retry_after(), Some(Duration::from_secs(7)));
        let body = br#"{"ok":false,"error":{"code":"rate_limited","hint":"slow","details":{"retry_after_ms":1500}}}"#;
        let raw = [
            format!("HTTP/1.1 429 X\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes(),
            body.to_vec(),
        ]
        .concat();
        let r = parse_response(&raw).unwrap();
        assert_eq!(r.retry_after(), Some(Duration::from_millis(1500)));
        assert_eq!(r.error_line(), "rate_limited: slow");
    }

    #[test]
    fn unreadable_or_empty_token_file_is_transport_class() {
        let dir = std::env::temp_dir().join(format!("k2-sew-tok-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("nope");
        let e = read_token(&missing).unwrap_err();
        assert!(e.0.starts_with("mgmt token file unreadable"), "{e}");
        let empty = dir.join("empty");
        std::fs::write(&empty, "  \n").unwrap();
        assert!(read_token(&empty).is_err());
        let ok = dir.join("ok");
        std::fs::write(&ok, "  s3cret \n").unwrap();
        assert_eq!(read_token(&ok).unwrap(), "s3cret");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
