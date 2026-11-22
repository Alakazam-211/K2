//! The `/storage/v1` door on an App's own hosts (PRD §10, lane D).
//!
//! supabase-js's `storage.from(bucket)` and the Dart storage client call
//! these with the App's host as the Supabase URL; nothing in the App
//! changes but the URL and the key.
//!
//! ```text
//! POST|PUT /storage/v1/object/<bucket>/<path>             upload (raw or multipart; x-upsert)
//! GET|HEAD /storage/v1/object/[authenticated/]<bucket>/<path>  download (Range)
//! GET      /storage/v1/object/info/[authenticated/]<bucket>/<path>  the row
//! DELETE   /storage/v1/object/<bucket>/<path>             remove one
//! DELETE   /storage/v1/object/<bucket>                    remove {prefixes}
//! POST     /storage/v1/object/list/<bucket>               list {prefix, limit, offset, sortBy, search}
//! POST     /storage/v1/object/move | copy                 {bucketId, sourceKey, destinationKey, destinationBucket?}
//! POST     /storage/v1/object/sign/<bucket>/<path>        {expiresIn} → {signedURL}
//! POST     /storage/v1/object/sign/<bucket>               {expiresIn, paths} → [{error, path, signedURL}]
//! GET      /storage/v1/object/sign/<bucket>/<path>?token= the signed download
//! POST     /storage/v1/object/upload/sign/<bucket>/<path> → {url, token}
//! PUT      /storage/v1/object/upload/sign/<bucket>/<path>?token=  the signed upload
//! GET|HEAD /storage/v1/object/public/<bucket>/<path>      a public bucket's object, no credential
//! GET|POST /storage/v1/bucket, GET|PUT|DELETE /storage/v1/bucket/<id>, POST …/<id>/empty
//! ```
//!
//! Not in v1 (PRD §17), each a 400 naming the part, never a wrong answer:
//! image transforms (`/render/image`), resumable TUS uploads
//! (`/upload/resumable`), the S3 protocol (`/s3`).
//!
//! **Where it answers.** On an App's addresses while compat is on for the
//! App (`storage_bind::watching`): `apps::voice_door` reads each request
//! head, as for the transfer door, and a `/storage/v1/` target is served
//! here, App up or down. Every answer closes the connection, so a later
//! request on a keep-alive connection can't slip past to the App (the
//! App path re-reads every request head on its rewrite path).
//!
//! **Who.** `storage_auth` (PRD §4): the session cookie, a third-party
//! token, the server key, or the anon lane. Signed URLs and public objects
//! need no credential: the token or the bucket's `public` is the grant.
//!
//! **Errors** are Supabase Storage's own (`storage_err`): HTTP 400 with the
//! real status in the body, so supabase-js and the Dart client read them
//! unchanged; the duplicate answer is exact.

use std::collections::HashMap;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf};

use super::door_config::DoorConfig;
use super::storage_auth::{self, AppFacts, ReqAuth, Via};
use super::storage_bytes::ByteStore;
use super::storage_err::{SResult, StorageError};
use super::storage_ops::{self as ops, Ctx, UploadMeta};
use super::storage_pg::Caller;
use super::storage_sign;

/// The door's namespace on an App's hosts.
pub const PREFIX: &str = "/storage/v1/";
/// A JSON body (everything but an upload) is at most this.
pub const MAX_JSON_BODY: usize = 1 << 20;
/// Multipart fields before the file, and each part's head.
const MAX_FIELD: usize = 64 * 1024;
/// Upload bytes must keep arriving: no gap longer than this.
const BODY_IDLE: Duration = Duration::from_secs(120);

/// True for a request target on the door (a query is fine).
pub fn is_door(target: &str) -> bool {
    let path = target.split_once('?').map_or(target, |(p, _)| p);
    path.starts_with(PREFIX) || path == "/storage/v1"
}

/// The head at the start of `bytes` when it is complete and its target is
/// on the door: `(head text, head length with its blank line)`.
pub fn door_head(bytes: &[u8]) -> Option<(String, usize)> {
    let len = bytes.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let head = String::from_utf8_lossy(&bytes[..len]).into_owned();
    let target = head.lines().next()?.split_whitespace().nth(1)?;
    is_door(target).then_some((head, len))
}

// ── the request ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Req {
    pub method: String,
    /// The path after `/storage/v1`, still percent-encoded.
    pub path: String,
    pub query: HashMap<String, String>,
    /// Header names lower-cased; the first of each.
    pub headers: HashMap<String, String>,
}

impl Req {
    pub fn parse(head: &str) -> Option<Req> {
        let mut lines = head.split("\r\n");
        let mut first = lines.next()?.split_whitespace();
        let method = first.next()?.to_ascii_uppercase();
        let target = first.next()?;
        let (path, q) = target.split_once('?').unwrap_or((target, ""));
        let path = path.strip_prefix("/storage/v1")?.to_string();
        let query = url::form_urlencoded::parse(q.as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
        let mut headers = HashMap::new();
        for l in lines {
            if let Some((k, v)) = l.split_once(':') {
                headers.entry(k.trim().to_ascii_lowercase()).or_insert_with(|| v.trim().to_string());
            }
        }
        Some(Req { method, path, query, headers })
    }

    pub fn h(&self, k: &str) -> Option<&str> {
        self.headers.get(k).map(String::as_str)
    }

    fn auth(&self) -> ReqAuth {
        ReqAuth {
            method: self.method.clone(),
            host: self.h("host").map(str::to_string),
            origin: self.h("origin").map(str::to_string),
            authorization: self.h("authorization").map(str::to_string),
            apikey: self.h("apikey").map(str::to_string),
            cookie: self.h("cookie").map(str::to_string),
        }
    }

    fn chunked(&self) -> bool {
        self.h("transfer-encoding").is_some_and(|t| t.to_ascii_lowercase().contains("chunked"))
    }

    fn content_length(&self) -> Option<u64> {
        self.h("content-length").and_then(|v| v.parse().ok())
    }
}

/// Percent-decode one path (`%2F` stays a slash in the name, as Supabase
/// reads it). `None` for bad escapes or non-UTF-8.
pub fn decode(p: &str) -> Option<String> {
    let b = p.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// What the request asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    BucketList,
    BucketCreate,
    Bucket(String),
    BucketEmpty(String),
    List(String),
    Move,
    Copy,
    SignOne(String, String),
    SignMany(String),
    SignedGet(String, String),
    UploadSign(String, String),
    UploadSigned(String, String),
    Public(String, String),
    Info(String, String),
    /// GET/HEAD/POST/PUT/DELETE on one object.
    Object(String, String),
    /// DELETE with `{prefixes}`.
    Remove(String),
    Unsupported(&'static str),
}

/// `<bucket>/<rest>` from an already-decoded tail.
fn split_bucket(t: &str) -> Option<(String, String)> {
    let (b, rest) = t.split_once('/')?;
    (!b.is_empty() && !rest.is_empty()).then(|| (b.to_string(), rest.to_string()))
}

pub fn route(method: &str, raw_path: &str) -> SResult<Route> {
    let path = decode(raw_path).ok_or_else(|| StorageError::invalid_request("the path is not valid percent-encoded UTF-8"))?;
    let p = path.trim_start_matches('/');
    let bad = || StorageError::new(404, "NotFound", "not_found", format!("Route {method}:/storage/v1/{p} not found"));
    if p.starts_with("render/") {
        return Ok(Route::Unsupported("image transformation"));
    }
    if p.starts_with("upload/resumable") {
        return Ok(Route::Unsupported("resumable (TUS) upload"));
    }
    if p.starts_with("s3") {
        return Ok(Route::Unsupported("the S3 protocol"));
    }
    if p == "bucket" || p == "bucket/" {
        return match method {
            "GET" => Ok(Route::BucketList),
            "POST" => Ok(Route::BucketCreate),
            _ => Err(bad()),
        };
    }
    if let Some(rest) = p.strip_prefix("bucket/") {
        if let Some(id) = rest.strip_suffix("/empty") {
            return Ok(Route::BucketEmpty(id.to_string()));
        }
        if rest.contains('/') {
            return Err(bad());
        }
        return Ok(Route::Bucket(rest.to_string()));
    }
    let Some(o) = p.strip_prefix("object/") else {
        return Err(bad());
    };
    if o == "move" {
        return Ok(Route::Move);
    }
    if o == "copy" {
        return Ok(Route::Copy);
    }
    if let Some(b) = o.strip_prefix("list/") {
        return Ok(Route::List(b.trim_end_matches('/').to_string()));
    }
    if let Some(t) = o.strip_prefix("upload/sign/") {
        let (b, n) = split_bucket(t).ok_or_else(bad)?;
        return Ok(if method == "PUT" { Route::UploadSigned(b, n) } else { Route::UploadSign(b, n) });
    }
    if let Some(t) = o.strip_prefix("sign/") {
        return Ok(match (method, split_bucket(t)) {
            ("GET" | "HEAD", Some((b, n))) => Route::SignedGet(b, n),
            ("POST", Some((b, n))) => Route::SignOne(b, n),
            ("POST", None) => Route::SignMany(t.trim_end_matches('/').to_string()),
            _ => return Err(bad()),
        });
    }
    if let Some(t) = o.strip_prefix("public/") {
        let (b, n) = split_bucket(t).ok_or_else(bad)?;
        return Ok(Route::Public(b, n));
    }
    if let Some(t) = o.strip_prefix("info/") {
        let t = t.strip_prefix("authenticated/").or_else(|| t.strip_prefix("public/")).unwrap_or(t);
        let (b, n) = split_bucket(t).ok_or_else(bad)?;
        return Ok(Route::Info(b, n));
    }
    if let Some(t) = o.strip_prefix("authenticated/") {
        let (b, n) = split_bucket(t).ok_or_else(bad)?;
        return Ok(Route::Object(b, n));
    }
    match split_bucket(o) {
        Some((b, n)) => Ok(Route::Object(b, n)),
        None if method == "DELETE" && !o.is_empty() => Ok(Route::Remove(o.trim_end_matches('/').to_string())),
        None => Err(bad()),
    }
}

// ── bodies ───────────────────────────────────────────────────────────

/// A request body: a `Content-Length` one, or `Transfer-Encoding: chunked`
/// decoded, over a buffered client. Every read waits at most [`BODY_IDLE`].
pub struct Body<'a, R> {
    inner: &'a mut BufReader<R>,
    mode: BodyMode,
    /// A chunk size line read so far (it can arrive split across reads).
    line: Vec<u8>,
}

enum BodyMode {
    Fixed(u64),
    /// Chunked: bytes left in this chunk, or `None` before a size line.
    Chunked(Option<u64>),
    Done,
}

impl<'a, R: AsyncRead + Unpin> Body<'a, R> {
    pub fn new(inner: &'a mut BufReader<R>, req: &Req) -> Self {
        let mode = if req.chunked() {
            BodyMode::Chunked(None)
        } else {
            match req.content_length() {
                Some(0) | None => BodyMode::Done,
                Some(n) => BodyMode::Fixed(n),
            }
        };
        Body { inner, mode, line: Vec::new() }
    }

    /// Read the whole body, at most `max` bytes.
    pub async fn read_all(&mut self, max: usize) -> SResult<Vec<u8>> {
        let mut out = Vec::new();
        let mut buf = [0u8; 16 * 1024];
        loop {
            let n = tokio::time::timeout(BODY_IDLE, self.read(&mut buf))
                .await
                .map_err(|_| StorageError::invalid_request("the request body stalled"))?
                .map_err(|e| StorageError::invalid_request(format!("the request body: {e}")))?;
            if n == 0 {
                return Ok(out);
            }
            if out.len() + n > max {
                return Err(StorageError::new(413, "EntityTooLarge", "Payload too large", format!("the request body is over {max} bytes")));
            }
            out.extend_from_slice(&buf[..n]);
        }
    }

    pub async fn read_json(&mut self) -> SResult<Value> {
        let b = self.read_all(MAX_JSON_BODY).await?;
        if b.iter().all(u8::is_ascii_whitespace) {
            return Ok(json!({}));
        }
        serde_json::from_slice(&b).map_err(|e| StorageError::invalid_request(format!("body is not JSON: {e}")))
    }
}

/// Read one CRLF line (a chunk size line) from a buffered reader.
fn poll_line<R: AsyncRead + Unpin>(r: &mut BufReader<R>, cx: &mut Context<'_>, acc: &mut Vec<u8>) -> Poll<std::io::Result<()>> {
    loop {
        let buf = match Pin::new(&mut *r).poll_fill_buf(cx) {
            Poll::Ready(Ok(b)) => b,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => return Poll::Pending,
        };
        if buf.is_empty() {
            return Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "chunked body ended early")));
        }
        match buf.iter().position(|&b| b == b'\n') {
            Some(i) => {
                acc.extend_from_slice(&buf[..=i]);
                Pin::new(&mut *r).consume(i + 1);
                return Poll::Ready(Ok(()));
            }
            None => {
                let n = buf.len();
                acc.extend_from_slice(buf);
                Pin::new(&mut *r).consume(n);
                if acc.len() > 4096 {
                    return Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "chunk size line too long")));
                }
            }
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for Body<'_, R> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, out: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let me = self.get_mut();
        loop {
            match me.mode {
                BodyMode::Done => return Poll::Ready(Ok(())),
                BodyMode::Fixed(0) => {
                    me.mode = BodyMode::Done;
                }
                BodyMode::Fixed(left) => {
                    let want = (left as usize).min(out.remaining());
                    let mut tmp = ReadBuf::new(out.initialize_unfilled_to(want));
                    match Pin::new(&mut *me.inner).poll_read(cx, &mut tmp) {
                        Poll::Ready(Ok(())) => {
                            let n = tmp.filled().len();
                            if n == 0 {
                                return Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "body shorter than Content-Length")));
                            }
                            out.advance(n);
                            me.mode = BodyMode::Fixed(left - n as u64);
                            return Poll::Ready(Ok(()));
                        }
                        other => return other,
                    }
                }
                BodyMode::Chunked(None) => {
                    match poll_line(me.inner, cx, &mut me.line) {
                        Poll::Ready(Ok(())) => {}
                        Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                        // What arrived so far stays in `me.line`.
                        Poll::Pending => return Poll::Pending,
                    }
                    let line = std::mem::take(&mut me.line);
                    let text = String::from_utf8_lossy(&line);
                    let size_txt = text.trim().split(';').next().unwrap_or("").trim().to_string();
                    if size_txt.is_empty() {
                        // The CRLF after a chunk's data.
                        continue;
                    }
                    let size = u64::from_str_radix(&size_txt, 16)
                        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad chunk size"));
                    match size {
                        Ok(0) => me.mode = BodyMode::Done,
                        Ok(n) => me.mode = BodyMode::Chunked(Some(n)),
                        Err(e) => return Poll::Ready(Err(e)),
                    }
                }
                BodyMode::Chunked(Some(0)) => me.mode = BodyMode::Chunked(None),
                BodyMode::Chunked(Some(left)) => {
                    let want = (left as usize).min(out.remaining());
                    let mut tmp = ReadBuf::new(out.initialize_unfilled_to(want));
                    match Pin::new(&mut *me.inner).poll_read(cx, &mut tmp) {
                        Poll::Ready(Ok(())) => {
                            let n = tmp.filled().len();
                            if n == 0 {
                                return Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "chunked body ended early")));
                            }
                            out.advance(n);
                            me.mode = BodyMode::Chunked(Some(left - n as u64));
                            return Poll::Ready(Ok(()));
                        }
                        other => return other,
                    }
                }
            }
        }
    }
}

/// Bytes of `inner` up to (not including) `delim`; then EOF. `pre`: bytes
/// already read past the part's head.
pub struct UntilDelim<R> {
    inner: R,
    delim: Vec<u8>,
    buf: Vec<u8>,
    found: bool,
    eof: bool,
}

impl<R: AsyncRead + Unpin> UntilDelim<R> {
    pub fn new(inner: R, delim: Vec<u8>, pre: Vec<u8>) -> Self {
        UntilDelim { inner, delim, buf: pre, found: false, eof: false }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

impl<R: AsyncRead + Unpin> AsyncRead for UntilDelim<R> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, out: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let me = self.get_mut();
        loop {
            if !me.found {
                if let Some(i) = find(&me.buf, &me.delim) {
                    me.buf.truncate(i);
                    me.found = true;
                }
            }
            let safe = if me.found { me.buf.len() } else { me.buf.len().saturating_sub(me.delim.len() - 1) };
            if safe > 0 {
                let n = safe.min(out.remaining());
                out.put_slice(&me.buf[..n]);
                me.buf.drain(..n);
                return Poll::Ready(Ok(()));
            }
            if me.found {
                return Poll::Ready(Ok(()));
            }
            if me.eof {
                return Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "multipart body ended before its closing boundary")));
            }
            let mut tmp = [0u8; 64 * 1024];
            let mut rb = ReadBuf::new(&mut tmp);
            match Pin::new(&mut me.inner).poll_read(cx, &mut rb) {
                Poll::Ready(Ok(())) => {
                    if rb.filled().is_empty() {
                        me.eof = true;
                    } else {
                        me.buf.extend_from_slice(rb.filled());
                    }
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// Read from `r` (with `carry` first) until `delim`; returns what came
/// before it and leaves what follows in `carry`. At most `cap` bytes.
async fn read_until<R: AsyncRead + Unpin>(r: &mut R, carry: &mut Vec<u8>, delim: &[u8], cap: usize) -> SResult<Vec<u8>> {
    let mut tmp = [0u8; 16 * 1024];
    loop {
        if let Some(i) = find(carry, delim) {
            let before = carry[..i].to_vec();
            carry.drain(..i + delim.len());
            return Ok(before);
        }
        if carry.len() > cap {
            return Err(StorageError::invalid_request("a multipart part's head or field is too long"));
        }
        let n = tokio::time::timeout(BODY_IDLE, r.read(&mut tmp))
            .await
            .map_err(|_| StorageError::invalid_request("the request body stalled"))?
            .map_err(|e| StorageError::invalid_request(format!("the request body: {e}")))?;
        if n == 0 {
            return Err(StorageError::new(400, "NoContentProvided", "NoContentProvided", "No content provided"));
        }
        carry.extend_from_slice(&tmp[..n]);
    }
}

/// A multipart upload up to its file part: the fields before it, the
/// file's content type, and the rest of the body.
pub struct Multipart {
    pub fields: HashMap<String, String>,
    pub file_type: Option<String>,
    pub carry: Vec<u8>,
    pub delim: Vec<u8>,
}

/// The `boundary=` of a multipart content type.
pub fn boundary(ct: &str) -> Option<String> {
    ct.split(';').find_map(|p| {
        let (k, v) = p.trim().split_once('=')?;
        k.trim().eq_ignore_ascii_case("boundary").then(|| v.trim().trim_matches('"').to_string())
    })
    .filter(|b| !b.is_empty() && b.len() <= 200)
}

fn disposition_param(d: &str, key: &str) -> Option<String> {
    d.split(';').skip(1).find_map(|p| {
        let (k, v) = p.trim().split_once('=')?;
        k.trim().eq_ignore_ascii_case(key).then(|| v.trim().trim_matches('"').to_string())
    })
}

/// Read a multipart body's fields up to its first file part.
pub async fn multipart_head<R: AsyncRead + Unpin>(r: &mut R, boundary: &str) -> SResult<Multipart> {
    let mut carry = Vec::new();
    let first = format!("--{boundary}");
    read_until(r, &mut carry, first.as_bytes(), MAX_FIELD).await?;
    let delim = format!("\r\n--{boundary}").into_bytes();
    let mut fields = HashMap::new();
    loop {
        // After a boundary: "--" ends the body, CRLF starts a part.
        let after = read_until(r, &mut carry, b"\r\n", MAX_FIELD).await?;
        if after.starts_with(b"--") {
            return Err(StorageError::new(400, "NoContentProvided", "NoContentProvided", "No content provided"));
        }
        let head = read_until(r, &mut carry, b"\r\n\r\n", MAX_FIELD).await?;
        let head = String::from_utf8_lossy(&head).into_owned();
        let mut disp = String::new();
        let mut ctype = None;
        for l in head.split("\r\n") {
            if let Some((k, v)) = l.split_once(':') {
                match k.trim().to_ascii_lowercase().as_str() {
                    "content-disposition" => disp = v.trim().to_string(),
                    "content-type" => ctype = Some(v.trim().to_string()),
                    _ => {}
                }
            }
        }
        if disposition_param(&disp, "filename").is_some() {
            return Ok(Multipart { fields, file_type: ctype, carry, delim });
        }
        let name = disposition_param(&disp, "name").unwrap_or_default();
        let value = read_until(r, &mut carry, &delim, MAX_FIELD).await?;
        fields.entry(name).or_insert_with(|| String::from_utf8_lossy(&value).into_owned());
    }
}

// ── answers ──────────────────────────────────────────────────────────

pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

fn reason(s: u16) -> &'static str {
    match s {
        200 => "OK",
        204 => "No Content",
        206 => "Partial Content",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        413 => "Payload Too Large",
        416 => "Range Not Satisfiable",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

impl Answer {
    pub fn json(status: u16, v: &Value) -> Self {
        Answer { status, headers: vec![("Content-Type".into(), "application/json".into())], body: v.to_string().into_bytes() }
    }
    pub fn err(e: &StorageError) -> Self {
        Self::json(e.http_status(), &e.body())
    }
}

/// The head of an answer; never a header value with a line break.
pub fn head_text(status: u16, headers: &[(String, String)], body_len: u64) -> String {
    let mut out = format!("HTTP/1.1 {status} {}\r\n", reason(status));
    for (k, v) in headers {
        if !k.contains(['\r', '\n']) && !v.contains(['\r', '\n']) {
            out.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    out.push_str(&format!("Content-Length: {body_len}\r\nConnection: close\r\n\r\n"));
    out
}

async fn send<S: AsyncWrite + Unpin>(client: &mut S, a: Answer, head_only: bool) {
    let mut bytes = head_text(a.status, &a.headers, a.body.len() as u64).into_bytes();
    if !head_only {
        bytes.extend_from_slice(&a.body);
    }
    let _ = client.write_all(&bytes).await;
    let _ = client.flush().await;
}

/// CORS for this request: a listed origin, or the host's own, is echoed
/// with credentials; a credential-free answer (public, signed) is `*`.
fn cors(req: &Req, cfg: &DoorConfig, open: bool) -> Vec<(String, String)> {
    let mut h = vec![(
        "Access-Control-Expose-Headers".to_string(),
        "Content-Length, Content-Range, ETag, Content-Type, Cache-Control, Last-Modified".to_string(),
    )];
    match req.h("origin") {
        Some(o) if storage_auth::origin_ok(o, req.h("host"), &cfg.origins) => {
            h.push(("Access-Control-Allow-Origin".into(), o.to_string()));
            h.push(("Access-Control-Allow-Credentials".into(), "true".into()));
            h.push(("Vary".into(), "Origin".into()));
        }
        _ if open => h.push(("Access-Control-Allow-Origin".into(), "*".into())),
        _ => {}
    }
    h
}

// ── the door ─────────────────────────────────────────────────────────

/// What the door needs for one App.
pub struct DoorApp {
    pub facts: AppFacts,
    pub cfg: DoorConfig,
    pub ctx: Ctx,
}

/// The live App: its service id, door settings and login. `None`: compat
/// is off (the App answers).
pub fn live_app(app_id: &str) -> SResult<Option<DoorApp>> {
    let Some(cfg) = super::storage_bind::config(app_id) else {
        return Ok(None);
    };
    let service_id = service_id_of(app_id).ok_or_else(|| StorageError::unavailable("the App is not installed here"))?;
    let target = super::storage_bind::authenticator_target(&cfg)?;
    let ctx = Ctx { app_id: app_id.to_string(), target, bytes: ByteStore::for_app(app_id) };
    Ok(Some(DoorApp { facts: AppFacts { app_id: app_id.to_string(), service_id }, cfg, ctx }))
}

fn service_id_of(app_id: &str) -> Option<String> {
    #[cfg(test)]
    if let Some(s) = test_service_id(app_id) {
        return Some(s);
    }
    live_service_id(app_id)
}

fn live_service_id(app_id: &str) -> Option<String> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    k2_core::deploy::store::get_app(&conn, app_id).ok().flatten().map(|a| a.service_id)
}

#[cfg(test)]
fn test_services() -> &'static std::sync::Mutex<HashMap<String, String>> {
    static T: std::sync::OnceLock<std::sync::Mutex<HashMap<String, String>>> = std::sync::OnceLock::new();
    T.get_or_init(Default::default)
}
#[cfg(test)]
fn test_service_id(app_id: &str) -> Option<String> {
    test_services().lock().unwrap_or_else(|e| e.into_inner()).get(app_id).cloned()
}
#[cfg(test)]
fn test_callers() -> &'static std::sync::Mutex<HashMap<String, Caller>> {
    static T: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Caller>>> = std::sync::OnceLock::new();
    T.get_or_init(Default::default)
}
/// Tests: an `Authorization` value → the caller it resolves to (the
/// credential rules themselves are `storage_auth`'s tests).
#[cfg(test)]
pub fn set_test_callers(m: HashMap<String, Caller>) {
    *test_callers().lock().unwrap_or_else(|e| e.into_inner()) = m;
}
/// Tests: `app_id`'s published service id, without a box.
#[cfg(test)]
pub fn set_test_service(app_id: &str, service_id: &str) {
    test_services().lock().unwrap_or_else(|e| e.into_inner()).insert(app_id.to_string(), service_id.to_string());
}

/// Serve one door request whose head is still unread on `client` (the
/// caller only peeked). The connection closes afterwards.
pub async fn serve<S: AsyncRead + AsyncWrite + Unpin>(client: &mut S, head: &str, head_len: usize, app_id: &str, via: Via) {
    let mut consumed = vec![0u8; head_len];
    if client.read_exact(&mut consumed).await.is_err() {
        return;
    }
    let app_o = app_id.to_string();
    let app = match tokio::task::spawn_blocking(move || live_app(&app_o)).await {
        Ok(Ok(Some(a))) => a,
        Ok(Ok(None)) => {
            return send(client, Answer::err(&StorageError::new(404, "NotFound", "not_found", "storage is not on for this App")), false).await;
        }
        Ok(Err(e)) => return send(client, Answer::err(&e), false).await,
        Err(e) => return send(client, Answer::err(&StorageError::internal(format!("worker: {e}"))), false).await,
    };
    serve_app(client, head, &app, via).await;
}

/// [`serve`] for an App already looked up (the head already consumed).
pub async fn serve_app<S: AsyncRead + AsyncWrite + Unpin>(client: &mut S, head: &str, app: &DoorApp, via: Via) {
    let Some(req) = Req::parse(head) else {
        return send(client, Answer::err(&StorageError::invalid_request("bad request line")), false).await;
    };
    let head_only = req.method == "HEAD";
    if req.method == "OPTIONS" {
        let mut h = cors(&req, &app.cfg, true);
        h.push(("Access-Control-Allow-Methods".into(), "GET, HEAD, POST, PUT, DELETE, OPTIONS".into()));
        h.push((
            "Access-Control-Allow-Headers".into(),
            "authorization, apikey, x-client-info, content-type, x-upsert, cache-control, x-metadata, range, if-none-match".into(),
        ));
        h.push(("Access-Control-Max-Age".into(), "600".into()));
        return send(client, Answer { status: 204, headers: h, body: vec![] }, false).await;
    }
    let mut buffered = BufReader::new(&mut *client);
    let result = dispatch(&mut buffered, &req, app, via).await;
    drop(buffered);
    let open = matches!(route(&req.method, &req.path), Ok(Route::Public(..) | Route::SignedGet(..)));
    match result {
        Ok(Out::Answer(mut a)) => {
            a.headers.extend(cors(&req, &app.cfg, open));
            send(client, a, head_only).await
        }
        Ok(Out::File(f)) => {
            let mut h = f.headers;
            h.extend(cors(&req, &app.cfg, open));
            let text = head_text(f.status, &h, f.len);
            if client.write_all(text.as_bytes()).await.is_err() || head_only {
                let _ = client.flush().await;
                return;
            }
            let Ok(mut file) = tokio::fs::File::open(&f.path).await else {
                return;
            };
            if file.seek(std::io::SeekFrom::Start(f.first)).await.is_err() {
                return;
            }
            let mut take = file.take(f.len);
            let _ = tokio::io::copy(&mut take, client).await;
            let _ = client.flush().await;
        }
        Err(e) => {
            let mut a = Answer::err(&e);
            a.headers.extend(cors(&req, &app.cfg, open));
            send(client, a, head_only).await
        }
    }
    let _ = client.shutdown().await;
}

/// A file answer, streamed after its head.
pub struct FileOut {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub path: std::path::PathBuf,
    pub first: u64,
    pub len: u64,
}

pub enum Out {
    Answer(Answer),
    File(FileOut),
}

fn ok(v: Value) -> SResult<Out> {
    Ok(Out::Answer(Answer::json(200, &v)))
}

async fn caller_of(app: &DoorApp, via: Via, req: &Req) -> SResult<Caller> {
    #[cfg(test)]
    if let Some(c) = req.h("authorization").and_then(|a| test_callers().lock().unwrap_or_else(|e| e.into_inner()).get(a).cloned()) {
        return Ok(c);
    }
    let (facts, cfg, auth) = (app.facts.clone(), app.cfg.clone(), req.auth());
    tokio::task::spawn_blocking(move || storage_auth::resolve(&facts, &cfg, via, &auth, &storage_auth::live_lookups()))
        .await
        .map_err(|e| StorageError::internal(format!("worker: {e}")))?
}

fn now() -> i64 {
    k2_core::deploy::now()
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str).filter(|x| !x.is_empty())
}

fn expires_in(v: &Value) -> SResult<i64> {
    v.get("expiresIn")
        .and_then(|x| x.as_i64().or_else(|| x.as_str().and_then(|s| s.parse().ok())))
        .ok_or_else(|| StorageError::invalid_request("body must have required property 'expiresIn'"))
}

/// The user metadata header (`x-metadata`: base64 JSON), as Supabase reads it.
fn user_metadata_header(req: &Req) -> Option<Value> {
    use base64::Engine as _;
    let raw = req.h("x-metadata")?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(raw.trim()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

async fn dispatch<R: AsyncRead + Unpin>(body: &mut BufReader<R>, req: &Req, app: &DoorApp, via: Via) -> SResult<Out> {
    let r = route(&req.method, &req.path)?;
    let ctx = &app.ctx;
    match r {
        Route::Unsupported(part) => Err(StorageError::unsupported(part)),
        // ── no credential: the token or the bucket is the grant ──
        Route::Public(bucket, name) => {
            let b = ops::bucket_info(ctx, &bucket).await?;
            if !b.public {
                return Err(StorageError::no_bucket());
            }
            let found = ops::find(ctx, &Caller::service("k2:public"), &bucket, &name).await?;
            file_answer(ctx, req, &name, &found).await
        }
        Route::SignedGet(bucket, name) => {
            let token = req.query.get("token").cloned().ok_or_else(|| StorageError::invalid_signature("Missing token"))?;
            let url = format!("{bucket}/{name}");
            let app_id = app.facts.app_id.clone();
            tokio::task::spawn_blocking(move || storage_sign::verify(&app_id, &token, &url, storage_sign::SCOPE_DOWNLOAD, now()))
                .await
                .map_err(|e| StorageError::internal(format!("worker: {e}")))??;
            let found = ops::find(ctx, &Caller::service("k2:signed"), &bucket, &name).await?;
            file_answer(ctx, req, &name, &found).await
        }
        Route::UploadSigned(bucket, name) => {
            let token = req.query.get("token").cloned().ok_or_else(|| StorageError::invalid_signature("Missing token"))?;
            let url = format!("{bucket}/{name}");
            let app_id = app.facts.app_id.clone();
            let claims = tokio::task::spawn_blocking(move || storage_sign::verify(&app_id, &token, &url, storage_sign::SCOPE_UPLOAD, now()))
                .await
                .map_err(|e| StorageError::internal(format!("worker: {e}")))??;
            let b = ops::bucket_info(ctx, &bucket).await?;
            upload_body(body, req, ctx, &b, &bucket, &name, claims.owner.as_deref(), claims.upsert.unwrap_or(false), None).await
        }
        // ── everything else runs as the caller ──
        other => {
            let caller = caller_of(app, via, req).await?;
            as_caller(body, req, app, &caller, other).await
        }
    }
}

async fn as_caller<R: AsyncRead + Unpin>(body: &mut BufReader<R>, req: &Req, app: &DoorApp, caller: &Caller, r: Route) -> SResult<Out> {
    let ctx = &app.ctx;
    let m = req.method.as_str();
    match (m, r) {
        ("GET", Route::BucketList) => ok(ops::bucket_list(ctx, caller).await?),
        ("POST", Route::BucketCreate) => {
            let v = Body::new(body, req).read_json().await?;
            let name = s(&v, "name").ok_or_else(|| StorageError::invalid_request("body must have required property 'name'"))?;
            let spec = ops::BucketSpec::from_body(&v)?;
            ok(ops::bucket_create(ctx, caller, s(&v, "id"), name, &spec).await?)
        }
        ("GET", Route::Bucket(id)) => ok(ops::bucket_get(ctx, caller, &id).await?),
        ("PUT", Route::Bucket(id)) => {
            let v = Body::new(body, req).read_json().await?;
            ok(ops::bucket_update(ctx, caller, &id, &ops::BucketSpec::from_body(&v)?).await?)
        }
        ("DELETE", Route::Bucket(id)) => ok(ops::bucket_delete(ctx, caller, &id).await?),
        ("POST", Route::BucketEmpty(id)) => ok(ops::bucket_empty(ctx, caller, &id).await?),
        ("POST", Route::List(bucket)) => {
            let v = Body::new(body, req).read_json().await?;
            ops::bucket_info(ctx, &bucket).await?;
            ok(ops::list(ctx, caller, &bucket, &ops::ListOpts::from_body(&v)?).await?)
        }
        ("POST", Route::Move) => {
            let v = Body::new(body, req).read_json().await?;
            let (b, from, to) = move_args(&v)?;
            let to_b = s(&v, "destinationBucket").unwrap_or(b);
            ok(ops::move_object(ctx, caller, b, from, to_b, to).await?)
        }
        ("POST", Route::Copy) => {
            let v = Body::new(body, req).read_json().await?;
            let (b, from, to) = move_args(&v)?;
            let to_b = s(&v, "destinationBucket").unwrap_or(b);
            let upsert = req.h("x-upsert") == Some("true");
            ok(ops::copy_object(ctx, caller, b, from, to_b, to, upsert).await?)
        }
        ("POST", Route::SignOne(bucket, name)) => {
            let v = Body::new(body, req).read_json().await?;
            let exp = expires_in(&v)?;
            ops::find(ctx, caller, &bucket, &name).await?;
            let url = format!("{bucket}/{name}");
            let (app_id, u) = (app.facts.app_id.clone(), url.clone());
            let token = tokio::task::spawn_blocking(move || storage_sign::sign_download(&app_id, &u, exp, now()))
                .await
                .map_err(|e| StorageError::internal(format!("worker: {e}")))??;
            ok(json!({ "signedURL": format!("/object/sign/{url}?token={token}") }))
        }
        ("POST", Route::SignMany(bucket)) => {
            let v = Body::new(body, req).read_json().await?;
            let exp = expires_in(&v)?;
            let paths: Vec<String> = v
                .get("paths")
                .and_then(Value::as_array)
                .ok_or_else(|| StorageError::invalid_request("body must have required property 'paths'"))?
                .iter()
                .map(|p| p.as_str().map(str::to_string).ok_or_else(|| StorageError::invalid_request("paths are strings")))
                .collect::<SResult<_>>()?;
            let seen = ops::readable(ctx, caller, &bucket, &paths).await?;
            let app_id = app.facts.app_id.clone();
            let b = bucket.clone();
            let out = tokio::task::spawn_blocking(move || -> SResult<Value> {
                let mut out = Vec::with_capacity(paths.len());
                for p in &paths {
                    if seen.contains(p) {
                        let t = storage_sign::sign_download(&app_id, &format!("{b}/{p}"), exp, now())?;
                        out.push(json!({ "error": null, "path": p, "signedURL": format!("/object/sign/{b}/{p}?token={t}") }));
                    } else {
                        out.push(json!({ "error": "Either the object does not exist or you do not have access to it", "path": p, "signedURL": null }));
                    }
                }
                Ok(Value::Array(out))
            })
            .await
            .map_err(|e| StorageError::internal(format!("worker: {e}")))??;
            ok(out)
        }
        ("POST", Route::UploadSign(bucket, name)) => {
            let upsert = req.h("x-upsert") == Some("true");
            let exp = req.query.get("expiresIn").and_then(|e| e.parse().ok()).unwrap_or(7200);
            ops::bucket_info(ctx, &bucket).await?;
            let meta = UploadMeta { upsert, mimetype: String::new(), cache_control: String::new(), user_metadata: None, size: None };
            ops::can_upload(ctx, caller, &bucket, &name, &meta).await?;
            let url = format!("{bucket}/{name}");
            let (app_id, u, owner) = (app.facts.app_id.clone(), url.clone(), caller.sub().map(str::to_string));
            let token = tokio::task::spawn_blocking(move || storage_sign::sign_upload(&app_id, &u, owner.as_deref(), upsert, exp, now()))
                .await
                .map_err(|e| StorageError::internal(format!("worker: {e}")))??;
            ok(json!({ "url": format!("/object/upload/sign/{url}?token={token}"), "token": token }))
        }
        ("GET" | "HEAD", Route::Info(bucket, name)) => ok(ops::info(ctx, caller, &bucket, &name).await?),
        ("GET" | "HEAD", Route::Object(bucket, name)) => {
            let found = ops::find(ctx, caller, &bucket, &name).await?;
            file_answer(ctx, req, &name, &found).await
        }
        ("POST" | "PUT", Route::Object(bucket, name)) => {
            let b = ops::bucket_info(ctx, &bucket).await?;
            // PUT is `update`: it replaces (Supabase's upsert), POST takes x-upsert.
            let upsert = m == "PUT" || req.h("x-upsert") == Some("true");
            upload_body(body, req, ctx, &b, &bucket, &name, caller.sub(), upsert, Some(caller)).await
        }
        ("DELETE", Route::Object(bucket, name)) => ok(ops::remove_one(ctx, caller, &bucket, &name).await?),
        ("DELETE", Route::Remove(bucket)) => {
            let v = Body::new(body, req).read_json().await?;
            let names: Vec<String> = v
                .get("prefixes")
                .and_then(Value::as_array)
                .ok_or_else(|| StorageError::invalid_request("body must have required property 'prefixes'"))?
                .iter()
                .map(|p| p.as_str().map(str::to_string).ok_or_else(|| StorageError::invalid_request("prefixes are strings")))
                .collect::<SResult<_>>()?;
            ok(ops::remove(ctx, caller, &bucket, &names).await?)
        }
        (m, r) => Err(StorageError::new(404, "NotFound", "not_found", format!("Route {m} not found for {r:?}"))),
    }
}

fn move_args(v: &Value) -> SResult<(&str, &str, &str)> {
    let b = s(v, "bucketId").ok_or_else(|| StorageError::invalid_request("body must have required property 'bucketId'"))?;
    let from = s(v, "sourceKey").ok_or_else(|| StorageError::invalid_request("body must have required property 'sourceKey'"))?;
    let to = s(v, "destinationKey").ok_or_else(|| StorageError::invalid_request("body must have required property 'destinationKey'"))?;
    Ok((b, from, to))
}

/// Read an upload's body (raw or multipart), check it, stage it, write
/// the row. `caller`: run the permission test as them first (a signed
/// upload URL's token already passed it when it was made).
#[allow(clippy::too_many_arguments)]
async fn upload_body<R: AsyncRead + Unpin>(
    body: &mut BufReader<R>,
    req: &Req,
    ctx: &Ctx,
    b: &ops::BucketInfo,
    bucket: &str,
    name: &str,
    owner: Option<&str>,
    upsert: bool,
    caller: Option<&Caller>,
) -> SResult<Out> {
    ops::must_key(name)?;
    let ct = req.h("content-type").unwrap_or("").to_string();
    let limit = b.limit();
    let mut raw = Body::new(body, req);
    if ct.to_ascii_lowercase().starts_with("multipart/form-data") {
        let bnd = boundary(&ct).ok_or_else(|| StorageError::invalid_request("multipart body without a boundary"))?;
        let mp = multipart_head(&mut raw, &bnd).await?;
        let mime = mp
            .fields
            .get("contentType")
            .cloned()
            .or(mp.file_type.clone())
            .unwrap_or_else(|| "application/octet-stream".into());
        let cache = match mp.fields.get("cacheControl").map(|s| s.trim()).filter(|s| !s.is_empty()) {
            Some(t) => format!("max-age={t}"),
            None => "no-cache".to_string(),
        };
        let um = mp.fields.get("metadata").or(mp.fields.get("userMetadata")).and_then(|m| serde_json::from_str(m).ok());
        let meta = UploadMeta { upsert, mimetype: mime, cache_control: cache, user_metadata: um, size: None };
        if let Some(c) = caller {
            ops::can_upload(ctx, c, bucket, name, &meta).await?;
        }
        if !b.mime_ok(&meta.mimetype) {
            return Err(StorageError::mime(&meta.mimetype));
        }
        let mut file = UntilDelim::new(&mut raw, mp.delim, mp.carry);
        let mut staged = ctx.bytes.stage(&mut file, limit).await?;
        return ok(ops::complete_upload(ctx, &mut staged, bucket, name, owner, &meta).await?);
    }
    let meta = UploadMeta {
        upsert,
        mimetype: if ct.is_empty() { "application/octet-stream".into() } else { ct.clone() },
        cache_control: req.h("cache-control").unwrap_or("no-cache").to_string(),
        user_metadata: user_metadata_header(req),
        size: req.content_length(),
    };
    if let Some(c) = caller {
        ops::can_upload(ctx, c, bucket, name, &meta).await?;
    }
    if !b.mime_ok(&meta.mimetype) {
        return Err(StorageError::mime(&meta.mimetype));
    }
    if req.content_length().is_some_and(|n| n > limit) {
        return Err(StorageError::too_large(limit));
    }
    let mut staged = ctx.bytes.stage(&mut raw, limit).await?;
    ok(ops::complete_upload(ctx, &mut staged, bucket, name, owner, &meta).await?)
}

/// `Range: bytes=a-b` for a file of `size`.
pub enum RangeAsk {
    Full,
    Part(u64, u64),
    Unsatisfiable,
}

pub fn parse_range(h: &str, size: u64) -> RangeAsk {
    let Some(spec) = h.trim().strip_prefix("bytes=") else {
        return RangeAsk::Full;
    };
    if spec.contains(',') {
        return RangeAsk::Full;
    }
    let Some((a, b)) = spec.split_once('-') else {
        return RangeAsk::Full;
    };
    let (a, b) = (a.trim(), b.trim());
    if size == 0 {
        return RangeAsk::Unsatisfiable;
    }
    match (a.parse::<u64>().ok(), b.parse::<u64>().ok()) {
        (Some(f), Some(l)) if f <= l && f < size => RangeAsk::Part(f, l.min(size - 1)),
        (Some(f), None) if b.is_empty() && f < size => RangeAsk::Part(f, size - 1),
        (None, Some(n)) if a.is_empty() && n > 0 => RangeAsk::Part(size.saturating_sub(n), size - 1),
        (Some(_), _) | (None, Some(_)) => RangeAsk::Unsatisfiable,
        _ => RangeAsk::Full,
    }
}

fn http_date(iso: &str) -> Option<String> {
    let t = chrono::DateTime::parse_from_rfc3339(iso).ok()?;
    Some(t.with_timezone(&chrono::Utc).format("%a, %d %b %Y %H:%M:%S GMT").to_string())
}

/// Serve a found object's bytes (Range, ETag, `?download`).
async fn file_answer(ctx: &Ctx, req: &Req, name: &str, f: &ops::Found) -> SResult<Out> {
    let version = f.version.clone().ok_or_else(StorageError::not_found)?;
    let path = ctx.bytes.object_path(&f.id, &version).ok_or_else(StorageError::not_found)?;
    let meta = tokio::fs::metadata(&path).await.map_err(|_| StorageError::not_found())?;
    let size = meta.len();
    let mut h: Vec<(String, String)> = vec![
        ("Content-Type".into(), f.mimetype()),
        ("Cache-Control".into(), f.cache_control()),
        ("Accept-Ranges".into(), "bytes".into()),
    ];
    if let Some(e) = f.etag() {
        h.push(("ETag".into(), e.clone()));
        if req.h("if-none-match").is_some_and(|inm| inm.split(',').any(|t| t.trim() == e)) {
            return Ok(Out::Answer(Answer { status: 304, headers: h, body: vec![] }));
        }
    }
    let lm = f.metadata.get("lastModified").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| f.updated_at.clone());
    if let Some(d) = http_date(&lm) {
        h.push(("Last-Modified".into(), d));
    }
    if let Some(dl) = req.query.get("download") {
        let file = if dl.is_empty() { name.rsplit('/').next().unwrap_or(name).to_string() } else { dl.clone() };
        let ascii: String = file.chars().map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' { c } else { '_' }).collect();
        h.push(("Content-Disposition".into(), format!("attachment; filename=\"{ascii}\";")));
    }
    let (status, first, len) = match req.h("range").map(|r| parse_range(r, size)).unwrap_or(RangeAsk::Full) {
        RangeAsk::Full => (200, 0, size),
        RangeAsk::Part(a, b) => {
            h.push(("Content-Range".into(), format!("bytes {a}-{b}/{size}")));
            (206, a, b - a + 1)
        }
        RangeAsk::Unsatisfiable => {
            h.push(("Content-Range".into(), format!("bytes */{size}")));
            return Err(StorageError::new(416, "InvalidRange", "InvalidRange", "The requested range is not satisfiable").with_http(416));
        }
    };
    Ok(Out::File(FileOut { status, headers: h, path, first, len }))
}

#[cfg(test)]
#[path = "storage_door_tests.rs"]
mod tests;
