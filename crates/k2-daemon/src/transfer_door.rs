//! The transfer door (0.46.9, `k2 study app-transfers`): plain HTTPS for an
//! OS background transfer (iOS `URLSession`, Android WorkManager), with a
//! short-lived token in `Authorization: Bearer` and nothing else.
//!
//! ```text
//! PUT    /_k2/transfers/uploads/<id>           Content-Range: bytes a-b/size, the bytes (≤ 16 MiB)
//! HEAD   /_k2/transfers/uploads/<id>           K2-Upload-Received / -Size / -Expires / -Done
//! GET    /_k2/transfers/uploads/<id>           the same as JSON
//! POST   /_k2/transfers/uploads/<id>/complete  {sha256?}: check and publish
//! DELETE /_k2/transfers/uploads/<id>           abort
//! GET    /_k2/transfers/downloads/<id>         the file: Range, If-Range, ETag (HEAD too)
//! OPTIONS (any of them)                        CORS preflight (bearer only, no cookies)
//! ```
//!
//! **Where.** On the box's own address (the dispatcher, never on an App's
//! gateway link) and on every App address while any transfer is live
//! (`apps::router` reads the first request head, as for the call door).
//! Every answer closes the connection, so the next request on an App's
//! keep-alive connection can't slip past to the App.
//!
//! **The token** (`k2up_…` / `k2dn_…`) opens its one upload or file and
//! nothing else; K2's own passes and cookies never count here. Before any
//! byte moves, the pass that made the transfer is read again by its id
//! (`skin::resolve_skin_token_id`) and its rights checked again: a revoked
//! pass is 401 `pass_invalid`, a lost cap, room or folder is 403
//! `cap_not_granted`. Per token: a rate limit; across the box: a cap on
//! requests at once.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_core::fs_transfer::{self as ft, TransferError};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};

/// Requests at once through the door, box-wide.
const MAX_IN_FLIGHT: usize = 64;
/// Per token: a burst, then a steady rate.
const RATE_BURST: f64 = 64.0;
const RATE_PER_SEC: f64 = 8.0;
const RATE_TRACKED_MAX: usize = 4096;
const RATE_IDLE: Duration = Duration::from_secs(600);
/// A range's bytes must arrive within this.
const BODY_TIMEOUT: Duration = Duration::from_secs(300);
/// `complete`'s own body (`{sha256}`).
const COMPLETE_BODY_MAX: usize = 4096;
/// How long "a transfer is live here" is trusted between reads.
const WATCH_TTL: Duration = Duration::from_secs(5);

/// True for a request target on the door (a query is fine).
pub fn is_door(path_and_query: &str) -> bool {
    path_and_query.starts_with(ft::DOOR_PREFIX)
}

/// The head at the start of `bytes` when it is complete and its target is on
/// the door: `(head text, head length with its blank line)`.
pub fn door_head(bytes: &[u8]) -> Option<(String, usize)> {
    let len = bytes.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let head = String::from_utf8_lossy(&bytes[..len]).into_owned();
    let target = head.lines().next()?.split_whitespace().nth(1)?;
    is_door(target).then_some((head, len))
}

/// Is any upload session or download grant on disk? Read at most every
/// [`WATCH_TTL`] (and set at once by [`note_live`]). An App's address only
/// reads its first request head while this is true.
pub fn watching() -> bool {
    let now = Instant::now();
    let mut g = watch().lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = *g {
        if now.duration_since(at) < WATCH_TTL {
            return v;
        }
    }
    let any = |d: PathBuf| std::fs::read_dir(d).map(|mut rd| rd.next().is_some()).unwrap_or(false);
    let v = any(ft::uploads_dir()) || any(ft::downloads_dir());
    *g = Some((now, v));
    v
}

fn watch() -> &'static Mutex<Option<(Instant, bool)>> {
    static W: OnceLock<Mutex<Option<(Instant, bool)>>> = OnceLock::new();
    W.get_or_init(Default::default)
}

/// A transfer was just made: App addresses start watching now.
pub fn note_live() {
    *watch().lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), true));
}

// ── limits ───────────────────────────────────────────────────────────

struct Bucket {
    tokens: f64,
    at: Instant,
}

fn take_token(key: &str, now: Instant) -> bool {
    static B: OnceLock<Mutex<HashMap<String, Bucket>>> = OnceLock::new();
    let mut g = B.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if g.len() >= RATE_TRACKED_MAX && !g.contains_key(key) {
        g.retain(|_, b| now.duration_since(b.at) < RATE_IDLE);
        if g.len() >= RATE_TRACKED_MAX {
            return false;
        }
    }
    let b = g.entry(key.to_string()).or_insert(Bucket { tokens: RATE_BURST, at: now });
    b.tokens = (b.tokens + now.duration_since(b.at).as_secs_f64() * RATE_PER_SEC).min(RATE_BURST);
    b.at = now;
    if b.tokens < 1.0 {
        return false;
    }
    b.tokens -= 1.0;
    true
}

static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

struct Slot;
impl Slot {
    fn take() -> Option<Slot> {
        if IN_FLIGHT.fetch_add(1, Ordering::SeqCst) >= MAX_IN_FLIGHT {
            IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Slot)
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

// ── answers ──────────────────────────────────────────────────────────

/// CORS for a bundled page (`tauri://localhost`) calling an App's address:
/// the token is a bearer, never a cookie, so any origin may send it.
const CORS: &str = "Access-Control-Allow-Origin: *\r\n\
Access-Control-Expose-Headers: ETag, Content-Range, Accept-Ranges, Content-Disposition, K2-Upload-Size, K2-Upload-Received, K2-Upload-Expires, K2-Upload-Done\r\n";

struct Answer {
    status: &'static str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Answer {
    fn json(status: &'static str, v: &Value) -> Self {
        Answer { status, headers: vec![("Content-Type".into(), "application/json".into())], body: v.to_string().into_bytes() }
    }
    fn refusal(status: &'static str, code: &str, hint: impl std::fmt::Display, extra: Option<Value>) -> Self {
        let mut v = json!({ "error": { "code": code, "hint": hint.to_string() } });
        if let Some(Value::Object(m)) = extra {
            for (k, x) in m {
                v[k] = x;
            }
        }
        Self::json(status, &v)
    }
    fn from_error(e: &TransferError) -> Self {
        let (status, code, extra) = crate::transfer_routes::refusal(e, true);
        Self::refusal(status, code, e, extra)
    }
    fn header(mut self, k: &str, v: impl Into<String>) -> Self {
        self.headers.push((k.to_string(), v.into()));
        self
    }
}

fn head_text(a: &Answer, body_len: u64) -> String {
    let mut out = format!("HTTP/1.1 {}\r\n{}", a.status, k2_core::frame_policy::CONNECT_HEADER_LINES);
    for (k, v) in &a.headers {
        // A value K2 wrote never holds a line break; refuse to send one.
        if !k.contains(['\r', '\n']) && !v.contains(['\r', '\n']) {
            out.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    out.push_str(&format!("Content-Length: {body_len}\r\nCache-Control: no-store\r\n{CORS}Connection: close\r\n\r\n"));
    out
}

async fn send<S: AsyncWrite + Unpin>(client: &mut S, a: Answer, head_only: bool) {
    let mut bytes = head_text(&a, a.body.len() as u64).into_bytes();
    if !head_only {
        bytes.extend_from_slice(&a.body);
    }
    let _ = client.write_all(&bytes).await;
    let _ = client.flush().await;
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|line| {
        let (n, v) = line.split_once(':')?;
        n.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// The bearer, or `?token=` (downloads only: a browser link can't set a
/// header).
fn credential<'a>(head: &'a str, query: Option<&'a str>, allow_query: bool) -> Option<String> {
    if let Some(b) = crate::routes::http::extract_bearer(head) {
        return Some(b.to_string());
    }
    if !allow_query {
        return None;
    }
    query?.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == "token").then(|| v.replace("%2E", ".").replace("%2e", "."))
    })
}

// ── the route ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route<'a> {
    Upload(&'a str),
    Complete(&'a str),
    Download(&'a str),
}

fn route(path: &str) -> Option<Route<'_>> {
    let rest = path.strip_prefix(ft::DOOR_PREFIX)?;
    let segs: Vec<&str> = rest.split('/').collect();
    match segs.as_slice() {
        ["uploads", id] if ft::is_id(id) => Some(Route::Upload(id)),
        ["uploads", id, "complete"] if ft::is_id(id) => Some(Route::Complete(id)),
        ["downloads", id] if ft::is_id(id) => Some(Route::Download(id)),
        _ => None,
    }
}

/// Serve one door request whose head is still unread on `client` (the
/// dispatcher only peeked; an App address replays what it read). The
/// connection closes afterwards.
pub async fn serve<S: AsyncRead + AsyncWrite + Unpin>(client: &mut S, head: &str, head_len: usize) {
    let mut consumed = vec![0u8; head_len];
    if client.read_exact(&mut consumed).await.is_err() {
        return;
    }
    let first = head.lines().next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return send(client, Answer::refusal("400 Bad Request", "usage", "bad request line", None), false).await;
    };
    let method = method.to_ascii_uppercase();
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (target, None),
    };
    let head_only = method == "HEAD";
    let Some(r) = route(path) else {
        return send(client, Answer::refusal("404 Not Found", "not_found", "no such transfer route", None), head_only).await;
    };
    if method == "OPTIONS" {
        let a = Answer { status: "204 No Content", headers: Vec::new(), body: Vec::new() }
            .header("Access-Control-Allow-Methods", "GET, HEAD, PUT, POST, DELETE, OPTIONS")
            .header("Access-Control-Allow-Headers", "Authorization, Content-Type, Content-Range, K2-Content-Sha256, Range, If-Range")
            .header("Access-Control-Max-Age", "600");
        return send(client, a, false).await;
    }
    if crate::routes::http::request_is_chunked(head) {
        return send(client, Answer::refusal("400 Bad Request", "usage", "send a Content-Length body, not chunked", None), head_only).await;
    }
    let is_download = matches!(r, Route::Download(_));
    let Some(token) = credential(head, query, is_download) else {
        let a = Answer::refusal("401 Unauthorized", "transfer_token_required", "this door takes a transfer token: Authorization: Bearer <token>", None)
            .header("WWW-Authenticate", "Bearer");
        return send(client, a, head_only).await;
    };
    let id = match r {
        Route::Upload(id) | Route::Complete(id) | Route::Download(id) => id.to_string(),
    };
    if !take_token(&id, Instant::now()) {
        let a = Answer::refusal("429 Too Many Requests", "rate_limited", "too many requests for this transfer", Some(json!({ "retryAfterSecs": 1 })))
            .header("Retry-After", "1");
        return send(client, a, head_only).await;
    }
    let Some(_slot) = Slot::take() else {
        let a = Answer::refusal("503 Service Unavailable", "busy", "too many transfers through this server right now", Some(json!({ "retryAfterSecs": 2 })))
            .header("Retry-After", "2");
        return send(client, a, head_only).await;
    };
    match r {
        Route::Upload(_) => serve_upload(client, head, &method, &id, &token).await,
        Route::Complete(_) => serve_complete(client, head, &method, &id, &token).await,
        Route::Download(_) => serve_download(client, head, &method, &id, &token).await,
    }
}

/// The session this token opens, its maker's pass read again and its
/// rights checked again.
fn upload_gate(id: &str, token: &str) -> Result<ft::UploadSession, Answer> {
    let now = ft::now_secs();
    let s = ft::authorize_upload(&ft::uploads_dir(), id, token, now).map_err(|e| Answer::from_error(&e))?;
    if s.done.is_some() {
        return Ok(s);
    }
    let pass_id = s.holder.pass_id.clone().unwrap_or_default();
    match k2_core::skin::resolve_skin_token_id(&pass_id) {
        Ok(Some(pass)) => {
            if crate::fs_routes::guest_upload_principal(&pass) != s.holder.principal
                || !crate::fs_routes::upload_still_allowed(&pass, &s.project_id, &s.rel_dir, s.drop)
            {
                let cap = if s.drop { k2_core::skin::CAP_FILES_UPLOAD } else { k2_core::skin::CAP_FILES_WRITE };
                return Err(Answer::refusal(
                    "403 Forbidden",
                    "cap_not_granted",
                    format!("the person no longer may upload here ({cap})"),
                    Some(json!({ "cap": cap })),
                ));
            }
            Ok(s)
        }
        Ok(None) => Err(Answer::refusal(
            "401 Unauthorized",
            "pass_invalid",
            "the pass that made this upload ended (signed out or revoked): sign in again and make a new upload",
            None,
        )),
        Err(e) => Err(Answer::refusal("500 Internal Server Error", "failed", e, None)),
    }
}

fn status_answer(s: &ft::UploadSession) -> Answer {
    Answer::json("200 OK", &s.status_json())
        .header("K2-Upload-Size", s.size.to_string())
        .header("K2-Upload-Received", ft::RangeSet::header(&s.received.0))
        .header("K2-Upload-Expires", s.expires_at.saturating_mul(1000).to_string())
        .header("K2-Upload-Done", if s.done.is_some() { "1" } else { "0" })
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(f).await.ok()
}

async fn serve_upload<S: AsyncRead + AsyncWrite + Unpin>(client: &mut S, head: &str, method: &str, id: &str, token: &str) {
    let head_only = method == "HEAD";
    let (id_o, tok_o) = (id.to_string(), token.to_string());
    let Some(gate) = blocking(move || upload_gate(&id_o, &tok_o)).await else {
        return send(client, Answer::refusal("500 Internal Server Error", "failed", "worker join", None), head_only).await;
    };
    let s = match gate {
        Ok(s) => s,
        Err(a) => return send(client, a, head_only).await,
    };
    match method {
        "GET" | "HEAD" => send(client, status_answer(&s), head_only).await,
        "DELETE" => {
            let id_o = id.to_string();
            let user = s.holder.username.clone();
            let r = blocking(move || ft::abort_upload(&ft::uploads_dir(), &k2_core::fs_guest_upload::ledger_dir(), &id_o)).await;
            match r {
                Some(Ok(())) => {
                    crate::thread_attachments::audit("files.upload_abort", &format!("guest:{user}"), format!("id={id} via=door"));
                    send(client, Answer::json("200 OK", &json!({ "ok": true, "uploadId": id, "aborted": true })), false).await
                }
                Some(Err(e)) => send(client, Answer::from_error(&e), false).await,
                None => send(client, Answer::refusal("500 Internal Server Error", "failed", "worker join", None), false).await,
            }
        }
        "PUT" => {
            let Some(cr) = header(head, "content-range") else {
                return send(client, Answer::refusal("400 Bad Request", "bad_range", "a PUT needs Content-Range: bytes <first>-<last>/<size>", None), false).await;
            };
            let (first, last, total) = match ft::parse_content_range(cr) {
                Ok(x) => x,
                Err(e) => return send(client, Answer::refusal("400 Bad Request", "bad_range", e, None), false).await,
            };
            let Some(len) = header(head, "content-length").and_then(|v| v.parse::<u64>().ok()) else {
                return send(client, Answer::refusal("411 Length Required", "usage", "a PUT needs Content-Length", None), false).await;
            };
            if len > ft::MAX_RANGE_LEN {
                return send(client, Answer::from_error(&TransferError::TooLarge(format!("one range carries at most {} bytes", ft::MAX_RANGE_LEN))), false).await;
            }
            if len != last - first + 1 {
                return send(client, Answer::refusal("400 Bad Request", "bad_range", "Content-Length is not the Content-Range's length", None), false).await;
            }
            let range_sha = match ft::parse_sha256(header(head, "k2-content-sha256")) {
                Ok(x) => x,
                Err(e) => return send(client, Answer::refusal("400 Bad Request", "usage", e, None), false).await,
            };
            let mut body = vec![0u8; len as usize];
            match tokio::time::timeout(BODY_TIMEOUT, client.read_exact(&mut body)).await {
                Ok(Ok(_)) => {}
                _ => return,
            }
            let id_o = id.to_string();
            let r = blocking(move || {
                ft::write_range(
                    &ft::uploads_dir(),
                    &k2_core::fs_guest_upload::ledger_dir(),
                    &id_o,
                    first,
                    total,
                    &body,
                    range_sha.as_deref(),
                    ft::now_secs(),
                )
            })
            .await;
            match r {
                Some(Ok(s)) => send(client, status_answer(&s), false).await,
                Some(Err(e)) => send(client, Answer::from_error(&e), false).await,
                None => send(client, Answer::refusal("500 Internal Server Error", "failed", "worker join", None), false).await,
            }
        }
        _ => send(client, Answer::refusal("405 Method Not Allowed", "usage", "PUT, HEAD, GET or DELETE", None), head_only).await,
    }
}

async fn serve_complete<S: AsyncRead + AsyncWrite + Unpin>(client: &mut S, head: &str, method: &str, id: &str, token: &str) {
    if method != "POST" {
        return send(client, Answer::refusal("405 Method Not Allowed", "usage", "POST", None), method == "HEAD").await;
    }
    let len = header(head, "content-length").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    if len > COMPLETE_BODY_MAX {
        return send(client, Answer::refusal("413 Payload Too Large", "too_large", "complete's body is {sha256} only", None), false).await;
    }
    let mut body = vec![0u8; len];
    match tokio::time::timeout(BODY_TIMEOUT, client.read_exact(&mut body)).await {
        Ok(Ok(_)) => {}
        _ => return,
    }
    let sha = if body.iter().all(u8::is_ascii_whitespace) {
        None
    } else {
        let v: Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => return send(client, Answer::refusal("400 Bad Request", "usage", format!("invalid JSON body: {e}"), None), false).await,
        };
        match ft::parse_sha256(v.get("sha256").and_then(|x| x.as_str())) {
            Ok(s) => s,
            Err(e) => return send(client, Answer::refusal("400 Bad Request", "usage", e, None), false).await,
        }
    };
    let (id_o, tok_o) = (id.to_string(), token.to_string());
    let r = blocking(move || -> Answer {
        let s = match upload_gate(&id_o, &tok_o) {
            Ok(s) => s,
            Err(a) => return a,
        };
        let who = format!("guest:{}", s.holder.username.trim());
        let resp = crate::transfer_routes::complete_answer(&ft::uploads_dir(), &k2_core::fs_guest_upload::ledger_dir(), &s, sha.as_deref(), &who);
        let body = resp.body.into_bytes();
        Answer { status: resp.status, headers: vec![("Content-Type".into(), resp.content_type.into())], body }
    })
    .await;
    match r {
        Some(a) => send(client, a, false).await,
        None => send(client, Answer::refusal("500 Internal Server Error", "failed", "worker join", None), false).await,
    }
}

/// The grant's file, its rights read again (see the module docs).
fn download_gate(id: &str, token: &str) -> Result<(ft::DownloadGrant, std::fs::File, std::fs::Metadata), Answer> {
    let g = ft::authorize_download(&ft::downloads_dir(), id, token, ft::now_secs()).map_err(|e| Answer::from_error(&e))?;
    let refused = |cap: &str| {
        Answer::refusal("403 Forbidden", "cap_not_granted", format!("the person no longer may save this file ({cap})"), Some(json!({ "cap": cap })))
    };
    let pass = match g.holder.pass_id.as_deref() {
        None => None,
        Some(pid) => match k2_core::skin::resolve_skin_token_id(pid) {
            Ok(Some(p)) if crate::fs_routes::guest_upload_principal(&p) == g.holder.principal => Some(p),
            Ok(_) => {
                return Err(Answer::refusal("401 Unauthorized", "pass_invalid", "the pass that asked for this download ended: ask again", None));
            }
            Err(e) => return Err(Answer::refusal("500 Internal Server Error", "failed", e, None)),
        },
    };
    let real = match (&g.recheck, &pass) {
        (ft::Recheck::Person, _) => dunce::canonicalize(&g.path).ok(),
        (ft::Recheck::File { workspace, rel_path }, Some(p)) => {
            let params: HashMap<String, String> =
                [("workspace".to_string(), workspace.clone()), ("path".to_string(), rel_path.clone())].into_iter().collect();
            crate::fs_download::resolve_target(&params, p).ok().map(|t| t.real)
        }
        (ft::Recheck::Attachment { project_id, rel_path }, Some(p)) => {
            crate::thread_attachments::guest_still_may_save(p, project_id, rel_path)
        }
        _ => None,
    };
    // Still allowed, and still the file the grant named (a path that now
    // lands elsewhere is not).
    let Some(real) = real.filter(|r| *r == g.path) else {
        return Err(refused(k2_core::skin::CAP_FILES_DOWNLOAD));
    };
    let f = std::fs::File::open(&real).map_err(|_| Answer::refusal("410 Gone", "transfer_expired", "the file is gone", None))?;
    let meta = f.metadata().map_err(|e| Answer::refusal("500 Internal Server Error", "failed", e, None))?;
    if !meta.is_file() {
        return Err(refused(k2_core::skin::CAP_FILES_DOWNLOAD));
    }
    Ok((g, f, meta))
}

/// `Content-Disposition` for a name (RFC 6266: an ASCII fallback plus
/// `filename*` in UTF-8).
fn disposition(name: &str) -> String {
    let ascii: String = name.chars().map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' { c } else { '_' }).collect();
    let enc: String = name
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect();
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{enc}")
}

async fn serve_download<S: AsyncRead + AsyncWrite + Unpin>(client: &mut S, head: &str, method: &str, id: &str, token: &str) {
    let head_only = method == "HEAD";
    if method != "GET" && method != "HEAD" {
        return send(client, Answer::refusal("405 Method Not Allowed", "usage", "GET or HEAD", None), false).await;
    }
    let (id_o, tok_o) = (id.to_string(), token.to_string());
    let Some(gate) = blocking(move || download_gate(&id_o, &tok_o)).await else {
        return send(client, Answer::refusal("500 Internal Server Error", "failed", "worker join", None), head_only).await;
    };
    let (g, file, meta) = match gate {
        Ok(x) => x,
        Err(a) => return send(client, a, head_only).await,
    };
    let size = meta.len();
    let etag = ft::file_etag(&meta);
    let ask = match header(head, "range") {
        Some(r) if ft::if_range_allows(header(head, "if-range"), &etag) => ft::parse_range(r, size),
        _ => ft::RangeAsk::Full,
    };
    let base = Answer { status: "200 OK", headers: Vec::new(), body: Vec::new() }
        .header("Content-Type", g.mime.clone())
        .header("ETag", etag.clone())
        .header("Accept-Ranges", "bytes")
        .header("Content-Disposition", disposition(&g.name));
    let (mut a, first, len) = match ask {
        ft::RangeAsk::Full => (base, 0, size),
        ft::RangeAsk::Part(f, l) => {
            let mut a = base.header("Content-Range", format!("bytes {f}-{l}/{size}"));
            a.status = "206 Partial Content";
            (a, f, l - f + 1)
        }
        ft::RangeAsk::Unsatisfiable => {
            let a = Answer::refusal("416 Range Not Satisfiable", "bad_range", format!("the file has {size} bytes"), None)
                .header("Content-Range", format!("bytes */{size}"))
                .header("ETag", etag);
            return send(client, a, head_only).await;
        }
    };
    if !head_only && first == 0 {
        crate::thread_attachments::audit(
            "files.download_door",
            &g.holder.username,
            format!("id={} name={:?} size={size} len={len}", g.id, g.name),
        );
    }
    a.body.clear();
    let head_bytes = head_text(&a, len);
    if client.write_all(head_bytes.as_bytes()).await.is_err() || head_only {
        let _ = client.flush().await;
        return;
    }
    let mut f = tokio::fs::File::from_std(file);
    if f.seek(std::io::SeekFrom::Start(first)).await.is_err() {
        return;
    }
    let mut take = f.take(len);
    let _ = tokio::io::copy(&mut take, client).await;
    let _ = client.flush().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_and_heads() {
        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(route(&format!("/_k2/transfers/uploads/{id}")), Some(Route::Upload(id)));
        assert_eq!(route(&format!("/_k2/transfers/uploads/{id}/complete")), Some(Route::Complete(id)));
        assert_eq!(route(&format!("/_k2/transfers/downloads/{id}")), Some(Route::Download(id)));
        assert_eq!(route("/_k2/transfers/uploads/../x"), None);
        assert_eq!(route("/_k2/transfers/uploads/ABC"), None);
        assert_eq!(route("/_k2/transfers/"), None);
        assert!(is_door("/_k2/transfers/uploads/x?y=1"));
        assert!(!is_door("/_k2/transfersx"));
        let raw = format!("PUT /_k2/transfers/uploads/{id} HTTP/1.1\r\nHost: a\r\n\r\nBODY");
        let (h, n) = door_head(raw.as_bytes()).expect("door head");
        assert!(h.starts_with("PUT "));
        assert_eq!(&raw.as_bytes()[n..], b"BODY");
        assert!(door_head(b"GET /api HTTP/1.1\r\n\r\n").is_none());
        assert!(door_head(b"GET /_k2/transfers/uploads/x HTTP/1.1\r\n").is_none(), "an incomplete head");
    }

    #[test]
    fn credentials_and_dispositions() {
        let head = "GET /x HTTP/1.1\r\nAuthorization: Bearer k2dn_abc.def\r\n\r\n";
        assert_eq!(credential(head, None, false).as_deref(), Some("k2dn_abc.def"));
        let bare = "GET /x HTTP/1.1\r\n\r\n";
        assert_eq!(credential(bare, Some("token=k2dn_a%2Eb"), true).as_deref(), Some("k2dn_a.b"));
        assert_eq!(credential(bare, Some("token=k2up_a.b"), false), None, "uploads take the header only");
        assert_eq!(disposition("r\u{e9}sum\u{e9} \"x\".pdf"), "attachment; filename=\"r_sum_ _x_.pdf\"; filename*=UTF-8''r%C3%A9sum%C3%A9%20%22x%22.pdf");
    }

    #[test]
    fn a_head_never_carries_a_line_break_from_a_value() {
        let a = Answer::json("200 OK", &json!({})).header("X-Evil", "a\r\nSet-Cookie: x=y");
        let h = head_text(&a, 2);
        assert!(!h.contains("Set-Cookie"), "{h}");
        assert!(h.contains("Connection: close\r\n\r\n"));
    }
}
