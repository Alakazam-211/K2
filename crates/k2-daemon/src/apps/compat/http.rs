//! HTTP/1.1 for the compat doors: read one request (head already peeked,
//! a `Content-Length` body), write one answer, keep the connection when the
//! client may send another.
//!
//! The doors answer on two kinds of connection: an App's public host (the
//! router hands the door a stream whose first bytes it already read) and
//! the App's gateway link (the dispatcher peeked the head). Both give the
//! door the head and its length; the door consumes exactly the request, so
//! the next request on a keep-alive connection starts where it left off.

use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The whole body arrives within this.
const BODY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// One request on a door.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DoorRequest {
    /// Upper case.
    pub method: String,
    /// The path, as sent (percent-encoded).
    pub path: String,
    /// The query string without `?`, as sent.
    pub query: String,
    /// Lower-case names, values trimmed, in order.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// HTTP/1.0, or `Connection: close`.
    pub close: bool,
}

impl DoorRequest {
    /// The first value of `name` (lower case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// `Authorization: Bearer <x>` → `x`.
    pub fn bearer(&self) -> Option<&str> {
        let v = self.header("authorization")?;
        let (scheme, rest) = v.split_once(' ')?;
        scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim()).filter(|s| !s.is_empty())
    }

    /// The cookie `name`'s value.
    pub fn cookie(&self, name: &str) -> Option<String> {
        self.headers.iter().filter(|(k, _)| k == "cookie").flat_map(|(_, v)| v.split(';')).find_map(|part| {
            let (k, v) = part.trim().split_once('=')?;
            (k.trim() == name).then(|| percent_decode(v.trim()))
        })
    }

    pub fn is_write(&self) -> bool {
        matches!(self.method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE")
    }

    /// The query's pairs, percent-decoded (`+` is a space, as in a form).
    pub fn query_pairs(&self) -> Vec<(String, String)> {
        query_pairs(&self.query)
    }
}

/// `a=1&b=x%20y` → `[("a","1"),("b","x y")]`. Empty pieces are skipped.
pub fn query_pairs(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (percent_decode(&k.replace('+', " ")), percent_decode(&v.replace('+', " "))),
            None => (percent_decode(&p.replace('+', " ")), String::new()),
        })
        .collect()
}

/// Percent-decode; a bad escape stays as it was. Invalid UTF-8 is replaced.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One answer.
#[derive(Debug, Clone, PartialEq)]
pub struct DoorAnswer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Close the connection after it (a request K2 could not read whole).
    pub close: bool,
}

impl DoorAnswer {
    pub fn new(status: u16) -> Self {
        DoorAnswer { status, headers: Vec::new(), body: Vec::new(), close: false }
    }

    pub fn json(status: u16, v: &Value) -> Self {
        DoorAnswer::new(status).header("Content-Type", "application/json; charset=utf-8").body(v.to_string().into_bytes())
    }

    /// PostgREST's error shape: `{code, message, details, hint}`.
    pub fn pgrst(status: u16, code: &str, message: impl Into<String>, details: Option<String>, hint: Option<String>) -> Self {
        DoorAnswer::json(status, &json!({ "code": code, "message": message.into(), "details": details, "hint": hint }))
    }

    pub fn header(mut self, k: &str, v: impl Into<String>) -> Self {
        self.headers.push((k.to_string(), v.into()));
        self
    }

    pub fn body(mut self, b: Vec<u8>) -> Self {
        self.body = b;
        self
    }

    pub fn closing(mut self) -> Self {
        self.close = true;
        self
    }

    pub fn has_header(&self, k: &str) -> bool {
        self.headers.iter().any(|(n, _)| n.eq_ignore_ascii_case(k))
    }
}

pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        206 => "Partial Content",
        300 => "Multiple Choices",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        416 => "Range Not Satisfiable",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Status",
    }
}

/// Why a request couldn't be read.
#[derive(Debug)]
pub enum ReadFail {
    /// The client went away (nothing to answer).
    Gone,
    /// Answer this, then close.
    Answer(DoorAnswer),
}

/// Parse a complete head (request line and headers, through the blank line).
pub fn parse_head(head: &str) -> Option<DoorRequest> {
    let mut lines = head.split("\r\n");
    let first = lines.next()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_ascii_uppercase();
    let target = parts.next()?;
    let version = parts.next().unwrap_or("HTTP/1.1");
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target.to_string(), String::new()),
    };
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        let (k, v) = line.split_once(':')?;
        headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
    }
    let conn_close = headers.iter().any(|(k, v)| k == "connection" && v.split(',').any(|t| t.trim().eq_ignore_ascii_case("close")));
    Some(DoorRequest { method, path, query, headers, body: Vec::new(), close: conn_close || version == "HTTP/1.0" })
}

/// Read one request whose head (`head_len` bytes, already peeked) is next on
/// `s`: consume the head, then the `Content-Length` body (at most
/// `max_body`). Chunked bodies are refused (411): the door can't find the
/// next request's start in a stream it doesn't decode.
pub async fn read_request<S: AsyncRead + Unpin>(s: &mut S, head: &str, head_len: usize, max_body: u64) -> Result<DoorRequest, ReadFail> {
    let mut consumed = vec![0u8; head_len];
    if s.read_exact(&mut consumed).await.is_err() {
        return Err(ReadFail::Gone);
    }
    let Some(mut req) = parse_head(head) else {
        return Err(ReadFail::Answer(DoorAnswer::pgrst(400, "PGRST000", "bad request head", None, None).closing()));
    };
    if req.header("transfer-encoding").is_some() {
        return Err(ReadFail::Answer(
            DoorAnswer::pgrst(411, "K2DOOR411", "send the body with a Content-Length, not chunked", None, None).closing(),
        ));
    }
    let len = match req.header("content-length") {
        None => 0,
        Some(v) => match v.parse::<u64>() {
            Ok(n) => n,
            Err(_) => return Err(ReadFail::Answer(DoorAnswer::pgrst(400, "PGRST000", "bad Content-Length", None, None).closing())),
        },
    };
    if len > max_body {
        return Err(ReadFail::Answer(
            DoorAnswer::pgrst(413, "K2DOOR413", format!("request bodies are at most {max_body} bytes"), None, None).closing(),
        ));
    }
    if len > 0 {
        let mut body = vec![0u8; len as usize];
        match tokio::time::timeout(BODY_TIMEOUT, s.read_exact(&mut body)).await {
            Ok(Ok(_)) => req.body = body,
            _ => return Err(ReadFail::Gone),
        }
    }
    Ok(req)
}

/// Write `a`. `head_only`: a HEAD request's answer (the length of the body
/// it would have had, no body). Returns whether the connection stays open.
pub async fn write_answer<S: AsyncWrite + Unpin>(s: &mut S, a: &DoorAnswer, head_only: bool, keep: bool) -> bool {
    let keep = keep && !a.close;
    let mut out = format!("HTTP/1.1 {} {}\r\n{}", a.status, reason(a.status), k2_core::frame_policy::CONNECT_HEADER_LINES);
    for (k, v) in &a.headers {
        // A value K2 wrote never holds a line break; never send one.
        if !k.contains(['\r', '\n', ':']) && !v.contains(['\r', '\n']) {
            out.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    if a.status != 204 && !(100..200).contains(&a.status) {
        out.push_str(&format!("Content-Length: {}\r\n", a.body.len()));
    }
    out.push_str(if keep { "Connection: keep-alive\r\n\r\n" } else { "Connection: close\r\n\r\n" });
    let mut bytes = out.into_bytes();
    if !head_only && a.status != 204 {
        bytes.extend_from_slice(&a.body);
    }
    if s.write_all(&bytes).await.is_err() || s.flush().await.is_err() {
        return false;
    }
    keep
}

/// The head at the start of `bytes`, when it is complete: `(text, length
/// with its blank line)`.
pub fn complete_head(bytes: &[u8]) -> Option<(String, usize)> {
    let len = bytes.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    Some((String::from_utf8_lossy(&bytes[..len]).into_owned(), len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heads_bearers_cookies_and_queries_parse() {
        let r = parse_head(
            "PATCH /rest/v1/notes?id=eq.1&select=a%2Cb HTTP/1.1\r\nHost: shop.example\r\nAuthorization: Bearer  k2skn_abc \r\nCookie: x=1; k2_skin_session=k2skn_%41B\r\n\r\n",
        )
        .unwrap();
        assert_eq!(r.method, "PATCH");
        assert_eq!(r.path, "/rest/v1/notes");
        assert_eq!(r.bearer(), Some("k2skn_abc"));
        assert_eq!(r.cookie("k2_skin_session").as_deref(), Some("k2skn_AB"));
        assert_eq!(r.cookie("nope"), None);
        assert!(r.is_write() && !r.close);
        assert_eq!(r.query_pairs(), vec![("id".to_string(), "eq.1".to_string()), ("select".to_string(), "a,b".to_string())]);
        let c = parse_head("GET / HTTP/1.0\r\n\r\n").unwrap();
        assert!(c.close);
        let c = parse_head("GET / HTTP/1.1\r\nConnection: keep-alive, Close\r\n\r\n").unwrap();
        assert!(c.close);
        assert_eq!(percent_decode("a%zz%2"), "a%zz%2");
        assert_eq!(percent_decode("%E2%9C%93"), "✓");
    }

    #[tokio::test]
    async fn a_request_is_consumed_exactly_and_the_next_one_is_left() {
        let raw = b"POST /rest/v1/t HTTP/1.1\r\nContent-Length: 5\r\n\r\nhelloGET /next HTTP/1.1\r\n\r\n".to_vec();
        let (head, len) = complete_head(&raw).unwrap();
        let mut s = std::io::Cursor::new(raw.clone());
        let r = read_request(&mut s, &head, len, 100).await.unwrap();
        assert_eq!(r.body, b"hello");
        let mut rest = Vec::new();
        s.read_to_end(&mut rest).await.unwrap();
        assert_eq!(rest, b"GET /next HTTP/1.1\r\n\r\n");
    }

    #[tokio::test]
    async fn chunked_and_oversize_bodies_are_refused_and_close() {
        let raw = b"POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        let (head, len) = complete_head(&raw).unwrap();
        match read_request(&mut std::io::Cursor::new(raw), &head, len, 100).await {
            Err(ReadFail::Answer(a)) => assert!(a.status == 411 && a.close),
            other => panic!("{other:?}"),
        }
        let raw = b"POST /x HTTP/1.1\r\nContent-Length: 101\r\n\r\n".to_vec();
        let (head, len) = complete_head(&raw).unwrap();
        match read_request(&mut std::io::Cursor::new(raw), &head, len, 100).await {
            Err(ReadFail::Answer(a)) => assert!(a.status == 413 && a.close),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn answers_carry_length_and_keep_alive_unless_closing() {
        let mut out = Vec::new();
        let keep = write_answer(&mut out, &DoorAnswer::json(200, &json!([1])), false, true).await;
        let text = String::from_utf8(out).unwrap();
        assert!(keep);
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "{text}");
        assert!(text.contains("Content-Length: 3\r\n") && text.contains("Connection: keep-alive") && text.ends_with("[1]"), "{text}");
        let mut out = Vec::new();
        let keep = write_answer(&mut out, &DoorAnswer::json(404, &json!({})).closing(), true, true).await;
        let text = String::from_utf8(out).unwrap();
        assert!(!keep && text.contains("Connection: close") && text.ends_with("\r\n\r\n"), "{text}");
    }
}
