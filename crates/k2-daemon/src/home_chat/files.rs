//! Attachments in Home DMs and channels (contract-home-chat-attachments-v1;
//! Sew 0.11.3's files: `sew-0.11.3-attachments-facts`).
//!
//! **Every byte goes through the daemon, as the person.** Sew listens on
//! this box's loopback only (a phone over Connect can't reach it), and its
//! signed URLs aren't tied to a viewer (whoever holds one fetches for about
//! an hour). So no client ever sees a Sew URL: each call here runs on the
//! person's own held Sew session ([`super::routes::sew_as`]), the same one
//! send and react use, and Sew decides what they may upload and read.
//!
//! - `POST uploads/create {conv, filename, size, contentType?}`: Sew's
//!   resumable upload on the conversation. The signed data URL Sew answers
//!   is kept here, per (person, upload), and never leaves the daemon.
//! - `POST uploads/data?id=` the next range's raw bytes (`Content-Range`,
//!   or `offset`), at most [`MAX_REQUEST_BYTES`], forwarded to that URL; an
//!   expired URL is renewed (`GET /api/v1/uploads/{id}`) once.
//! - `GET uploads/status?id=` (resume after a cut), `POST uploads/abort`.
//! - `send {…, attachments:[upload id…]}` ([`send_ids`]): Sew's `upload_ids`.
//! - `GET file?id=&v=original|thumb|preview|poster[&dl=1]`: the bytes,
//!   streamed ([`open`], [`stream_out`]), `Range` and validators passed to
//!   Sew. Sew is asked for the file as the person on every fetch (its
//!   metadata kept at most [`META_TTL`] per person), so a login that can't
//!   read the message gets 404.
//!
//! **Served as attachments.** There is no virus scanning anywhere: every
//! download is `Content-Disposition: attachment` with `nosniff` and a
//! sandbox CSP, except Sew's own renders (thumbnail, preview, poster) and
//! PNG, JPEG, GIF or WebP originals, which are `inline`.
//!
//! **On messages** ([`scrub`]): every `attachments[]` K2 answers or sends
//! on a socket carries K2's file paths in Sew's URL fields, never Sew's.

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

use super::people::Caller;
use super::routes::{ok_or_refuse, param, sew_as, sew_id};
use super::{deps, follower, now_secs};
use crate::voice::bff::{Refusal, SewAnswer, SewBytes};

type R<T> = Result<T, Refusal>;

pub const FILE: &str = "/cli/home-chat/file";
pub const CREATE: &str = "/cli/home-chat/uploads/create";
pub const DATA: &str = "/cli/home-chat/uploads/data";
pub const STATUS: &str = "/cli/home-chat/uploads/status";
pub const ABORT: &str = "/cli/home-chat/uploads/abort";

/// Most files on one message (Sew's `upload_ids` cap).
pub const PER_MESSAGE: usize = 10;
/// Sew's chunk: each range ends on a multiple of it (except the last).
pub const CHUNK_BYTES: u64 = 1 << 20;
/// Most bytes in one `uploads/data` request (Sew takes 16 MiB; 8 keeps the
/// daemon's memory per request small).
pub const MAX_REQUEST_BYTES: usize = 8 << 20;
/// The params key the dispatcher puts a request's `Content-Range` under.
pub const RANGE_PARAM: &str = "content_range";
/// How long a file's metadata (its signed URLs) is kept for one person.
pub const META_TTL: Duration = Duration::from_secs(60);
/// A thumbnail, preview or poster is the same bytes for as long as its id
/// lives (Sew renders it once): the phone keeps it this long.
pub const RENDER_MAX_AGE_SECS: u64 = 30 * 24 * 3600;
const META_MAX: usize = 2048;
const UPLOADS_MAX: usize = 4096;
/// An upload URL this close to its expiry is renewed before use.
const URL_MARGIN_SECS: i64 = 60;

/// Only these render where they are; anything else is a download.
const SAFE_INLINE: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];
/// The CSP on every file K2 serves.
pub const FILE_CSP: &str = "default-src 'none'; sandbox";

fn usage(hint: impl Into<String>) -> Refusal {
    Refusal::new(400, "usage", hint)
}

fn unavailable(e: impl std::fmt::Display) -> Refusal {
    super::sessions::unavailable(e)
}

pub fn chunk_too_large() -> Refusal {
    Refusal::new(413, "chunk_too_large", format!("one request carries at most {MAX_REQUEST_BYTES} bytes: send the file in ranges"))
        .details(json!({"max": MAX_REQUEST_BYTES}))
}

fn too_large(max: Value) -> Refusal {
    Refusal::new(413, "too_large", "this file is larger than this server takes").details(json!({"max": max}))
}

fn upload_not_found() -> Refusal {
    Refusal::new(404, "upload_not_found", "no upload of yours with that id (it expired, was aborted, or isn't yours)")
}

fn file_not_found() -> Refusal {
    Refusal::new(404, "file_not_found", "no file with that id that you can read")
}

fn id_str(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Sew's refusal, with `413 too_large`'s `max` at the top of `details`.
fn sew_ok(a: SewAnswer) -> R<Value> {
    if !a.ok() && a.body.pointer("/error/code").and_then(Value::as_str) == Some("too_large") {
        return Err(too_large(a.body.pointer("/error/details/max").cloned().unwrap_or(Value::Null)));
    }
    ok_or_refuse(a)
}

// ── limits ───────────────────────────────────────────────────────────

/// The person's largest upload, as their gateway's READY said it.
pub fn max_bytes(ext: &str) -> Option<u64> {
    follower::get(ext).and_then(|h| h.state.lock().unwrap_or_else(|p| p.into_inner()).files_max_bytes)
}

/// `/cli/home-chat/status`'s `files`.
pub fn limits(ext: &str) -> Value {
    json!({
        "maxBytes": max_bytes(ext),
        "perMessage": PER_MESSAGE,
        "chunkBytes": CHUNK_BYTES,
        "maxRequestBytes": MAX_REQUEST_BYTES,
    })
}

/// `send`'s `attachments`: Sew ids, at most [`PER_MESSAGE`], no repeats.
pub fn send_ids(ids: &[String]) -> R<Vec<String>> {
    if ids.len() > PER_MESSAGE {
        return Err(Refusal::new(400, "too_many_files", format!("a message takes at most {PER_MESSAGE} files")).details(json!({"max": PER_MESSAGE})));
    }
    let mut out: Vec<String> = Vec::with_capacity(ids.len());
    for id in ids {
        let id = sew_id(id, "each attachment (an upload id)")?;
        if out.contains(&id) {
            return Err(usage("an upload is attached once"));
        }
        out.push(id);
    }
    Ok(out)
}

// ── uploads ──────────────────────────────────────────────────────────

/// One upload K2 holds the signed data URL of, for the person who made it.
#[derive(Debug, Clone)]
struct Up {
    /// Sew's signed data path (`/api/v1/uploads/{id}/data?exp=&sig=`).
    path: String,
    /// When that path stops working (epoch seconds).
    url_exp: i64,
    size: u64,
}

type UpKey = (String, String);

fn uploads() -> &'static Mutex<HashMap<UpKey, Up>> {
    static U: OnceLock<Mutex<HashMap<UpKey, Up>>> = OnceLock::new();
    U.get_or_init(Default::default)
}

/// Tests: forget every upload URL and file kept (a daemon restart).
#[doc(hidden)]
pub fn forget_for_tests() {
    uploads().lock().unwrap_or_else(|p| p.into_inner()).clear();
    metas().lock().unwrap_or_else(|p| p.into_inner()).clear();
    let mut m = memo().lock().unwrap_or_else(|p| p.into_inner());
    m.map.clear();
    m.order.clear();
}

/// The path and query of a Sew URL (path-absolute, or absolute on any
/// host: the bytes always go to Sew's loopback listener). Only Sew's API.
pub fn signed_path(url: &str) -> Option<String> {
    let rest = match url.strip_prefix("http://").or_else(|| url.strip_prefix("https://")) {
        Some(r) => &r[r.find('/')?..],
        None => url,
    };
    let ok = rest.starts_with("/api/v1/") && !rest.contains("..") && !rest.chars().any(|c| c.is_control() || c == ' ' || c == '#');
    ok.then(|| rest.to_string())
}

fn epoch_of(v: &Value) -> Option<i64> {
    v.as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).map(|t| t.timestamp())
}

/// Keep Sew's answer's `upload_url` for (person, upload).
fn keep(ext: &str, id: &str, b: &Value) -> R<Up> {
    let path = b["upload_url"].as_str().and_then(signed_path).ok_or_else(|| unavailable("Sew's upload answer had no data URL"))?;
    let up = Up {
        path,
        // No expiry said: use it for a few minutes, then ask again.
        url_exp: epoch_of(&b["url_expires_at"]).unwrap_or_else(|| now_secs() + 300),
        size: b["size"].as_u64().unwrap_or(0),
    };
    let mut m = uploads().lock().unwrap_or_else(|p| p.into_inner());
    if m.len() >= UPLOADS_MAX {
        let now = now_secs();
        m.retain(|_, u| u.url_exp > now);
        if m.len() >= UPLOADS_MAX {
            if let Some(k) = m.iter().min_by_key(|(_, u)| u.url_exp).map(|(k, _)| k.clone()) {
                m.remove(&k);
            }
        }
    }
    m.insert((ext.to_string(), id.to_string()), up.clone());
    Ok(up)
}

fn forget(ext: &str, id: &str) {
    uploads().lock().unwrap_or_else(|p| p.into_inner()).remove(&(ext.to_string(), id.to_string()));
}

/// Sew's upload as the person (`GET /api/v1/uploads/{id}`, the uploader
/// only): its status and a fresh data URL, kept.
fn sew_upload(c: &Caller, id: &str) -> R<Value> {
    let a = sew_as(&c.ext, "GET", &format!("/api/v1/uploads/{id}"), None)?;
    if a.status == 404 || a.status == 403 {
        forget(&c.ext, id);
        return Err(upload_not_found());
    }
    let v = ok_or_refuse(a)?;
    keep(&c.ext, id, &v)?;
    Ok(v)
}

/// The person's upload `id`: the kept data URL while it has a minute left,
/// else a fresh one from Sew. Never another person's: the key is theirs.
fn entry(c: &Caller, id: &str, fresh: bool) -> R<Up> {
    if !fresh {
        let m = uploads().lock().unwrap_or_else(|p| p.into_inner());
        if let Some(u) = m.get(&(c.ext.clone(), id.to_string())).filter(|u| u.url_exp > now_secs() + URL_MARGIN_SECS) {
            return Ok(u.clone());
        }
    }
    let v = sew_upload(c, id)?;
    let m = uploads().lock().unwrap_or_else(|p| p.into_inner());
    m.get(&(c.ext.clone(), id.to_string())).cloned().ok_or_else(|| unavailable(format!("upload {id}: {}", v["upload_id"])))
}

/// What a client sees of an upload (never its URL).
fn view(id: &str, b: &Value) -> Value {
    json!({
        "id": id,
        "committed": b["committed_bytes"].as_u64().unwrap_or(0),
        "size": b["size"],
        "complete": b["complete"].as_bool().unwrap_or(false),
        "expiresAt": b["expires_at"],
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct CreateBody {
    conv: String,
    filename: String,
    size: u64,
    #[serde(default)]
    content_type: Option<String>,
}

/// `POST uploads/create`: Sew's resumable upload on the conversation.
pub(super) fn create(c: &Caller, b: CreateBody) -> R<Value> {
    let conv = sew_id(&b.conv, "conv")?;
    let name = b.filename.trim();
    if name.is_empty() || name.len() > 1024 || name.chars().any(char::is_control) {
        return Err(usage("filename is 1-255 characters, no control characters"));
    }
    if b.size == 0 {
        return Err(usage("an empty file can't be attached"));
    }
    if let Some(max) = max_bytes(&c.ext) {
        if b.size > max {
            return Err(too_large(json!(max)));
        }
    }
    let ct = b.content_type.as_deref().map(str::trim).filter(|t| !t.is_empty());
    if let Some(t) = ct {
        if t.len() > 255 || !t.contains('/') || t.chars().any(|ch| ch.is_control() || ch == ' ') {
            return Err(usage("contentType is a MIME type (type/subtype)"));
        }
    }
    super::routes::check_dm_writable(c, &conv)?;
    super::routes::check_channel_writable(&conv)?;
    let mut body = json!({"filename": name, "size": b.size});
    if let Some(t) = ct {
        body["content_type"] = json!(t);
    }
    let a = sew_as(&c.ext, "POST", &format!("/api/v1/channels/{conv}/uploads"), Some(&body))?;
    if a.status == 404 {
        if let Some(gone) = super::routes::dm_gone(c, &conv)? {
            return Err(gone);
        }
    }
    let v = sew_ok(a)?;
    let id = id_str(&v["upload_id"]).ok_or_else(|| unavailable("Sew's upload answer had no id"))?;
    keep(&c.ext, &id, &v)?;
    let mut up = view(&id, &v);
    up["conv"] = json!(conv);
    up["filename"] = json!(name);
    up["chunkBytes"] = json!(v["chunk_size"].as_u64().unwrap_or(CHUNK_BYTES));
    up["maxRequestBytes"] = json!(MAX_REQUEST_BYTES);
    Ok(json!({"ok": true, "upload": up}))
}

/// `bytes a-b/total` → (a, b, total).
pub fn parse_content_range(h: &str) -> Option<(u64, u64, u64)> {
    let r = h.trim().strip_prefix("bytes ")?;
    let (span, total) = r.split_once('/')?;
    let (a, b) = span.split_once('-')?;
    let (a, b, t): (u64, u64, u64) = (a.trim().parse().ok()?, b.trim().parse().ok()?, total.trim().parse().ok()?);
    (a <= b && b < t).then_some((a, b, t))
}

/// `POST uploads/data?id=`: the next range of the person's upload.
pub(super) fn data(c: &Caller, p: &HashMap<String, String>, body: &[u8]) -> R<Value> {
    let id = sew_id(param(p, "id").ok_or_else(|| usage("name the upload: id"))?, "id")?;
    if body.is_empty() {
        return Err(usage("no bytes: the body is the file's next range"));
    }
    if body.len() > MAX_REQUEST_BYTES {
        return Err(chunk_too_large());
    }
    let len = body.len() as u64;
    let up = entry(c, &id, false)?;
    let (start, end, total) = match param(p, RANGE_PARAM) {
        Some(h) => parse_content_range(h).ok_or_else(|| usage("Content-Range is bytes <first>-<last>/<size>"))?,
        None => {
            let off: u64 = param(p, "offset")
                .ok_or_else(|| usage("say where the bytes go: Content-Range, or offset"))?
                .parse()
                .map_err(|_| usage("offset is a byte offset"))?;
            (off, off.checked_add(len - 1).ok_or_else(|| usage("offset is too large"))?, up.size)
        }
    };
    if end - start + 1 != len {
        return Err(usage("the range's length is not the body's"));
    }
    if up.size != 0 && total != up.size {
        return Err(usage(format!("the range's size is not the upload's ({})", up.size)));
    }
    let range = format!("bytes {start}-{end}/{total}");
    let send = |u: &Up| deps().sew.post_bytes(&u.path, &range, body).map_err(unavailable);
    let mut a = send(&up)?;
    // A data URL past its time (Sew's 403 url_expired or url_invalid): a
    // fresh one, once.
    if a.status == 401 || a.status == 403 || a.status == 410 {
        a = send(&entry(c, &id, true)?)?;
    }
    if a.status == 404 {
        forget(&c.ext, &id);
        return Err(upload_not_found());
    }
    let v = sew_ok(a)?;
    Ok(json!({"ok": true, "upload": view(&id, &v)}))
}

/// `GET uploads/status?id=`: where the person's upload stands (resume
/// after a cut from `committed`).
pub(super) fn status(c: &Caller, p: &HashMap<String, String>) -> R<Value> {
    let id = sew_id(param(p, "id").ok_or_else(|| usage("name the upload: id"))?, "id")?;
    let v = sew_upload(c, &id)?;
    let mut up = view(&id, &v);
    up["conv"] = json!(id_str(&v["channel_id"]));
    up["filename"] = v["filename"].clone();
    up["chunkBytes"] = json!(v["chunk_size"].as_u64().unwrap_or(CHUNK_BYTES));
    up["maxRequestBytes"] = json!(MAX_REQUEST_BYTES);
    Ok(json!({"ok": true, "upload": up}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AbortBody {
    id: String,
}

/// `POST uploads/abort {id}`.
pub(super) fn abort(c: &Caller, b: AbortBody) -> R<Value> {
    let id = sew_id(&b.id, "id")?;
    let a = sew_as(&c.ext, "POST", &format!("/api/v1/uploads/{id}/abort"), Some(&json!({})))?;
    forget(&c.ext, &id);
    if a.status == 404 || a.status == 403 {
        return Err(upload_not_found());
    }
    ok_or_refuse(a)?;
    Ok(json!({"ok": true}))
}

// ── attachments on messages ──────────────────────────────────────────

/// Sew's URL fields and the variant each one is.
const URL_FIELDS: &[(&str, &str)] = &[("url", "original"), ("thumbnail_url", "thumb"), ("preview_url", "preview"), ("poster_url", "poster")];

/// K2's path for a file's variant.
pub fn file_path(id: &str, variant: &str) -> String {
    if variant == "original" {
        format!("{FILE}?id={id}")
    } else {
        format!("{FILE}?id={id}&v={variant}")
    }
}

/// One attachment as K2 serves it: Sew's URL fields hold K2's paths (only
/// the variants Sew has), and `url_expires_at` is gone (K2's never expire).
fn scrub_one(a: &mut Value) {
    let Some(o) = a.as_object_mut() else { return };
    let id = o.get("id").and_then(id_str).filter(|i| k2_core::tool_hook::sew_wire::is_sew_id(i));
    for (field, variant) in URL_FIELDS {
        let has = o.get(*field).is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty()));
        match (&id, has) {
            (Some(id), true) => {
                o.insert((*field).to_string(), json!(file_path(id, variant)));
            }
            _ => {
                o.remove(*field);
            }
        }
    }
    o.remove("url_expires_at");
}

/// Every `attachments` list in `v` (a message, a page, a changes feed, a
/// frame), wherever it sits, as K2 serves it: no Sew URL ever leaves.
pub fn scrub(v: &mut Value) {
    match v {
        Value::Object(o) => {
            for (k, x) in o.iter_mut() {
                if k == "attachments" {
                    if let Value::Array(list) = &mut *x {
                        for a in list.iter_mut() {
                            scrub_one(a);
                        }
                        continue;
                    }
                }
                scrub(x);
            }
        }
        Value::Array(list) => list.iter_mut().for_each(scrub),
        _ => {}
    }
}

// ── a file's bytes ───────────────────────────────────────────────────

type MetaKey = (String, String);

fn metas() -> &'static Mutex<HashMap<MetaKey, (Instant, Value)>> {
    static M: OnceLock<Mutex<HashMap<MetaKey, (Instant, Value)>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

/// The attachment `id` as person `c` may read it (Sew's `GET
/// /api/v1/files/{id}` on their session: 404 when they can't), kept for
/// [`META_TTL`] per person.
fn meta(c: &Caller, id: &str, fresh: bool) -> R<Value> {
    let key = (c.ext.clone(), id.to_string());
    if !fresh {
        let m = metas().lock().unwrap_or_else(|p| p.into_inner());
        if let Some((_, v)) = m.get(&key).filter(|(at, _)| at.elapsed() < META_TTL) {
            return Ok(v.clone());
        }
    }
    let a = sew_as(&c.ext, "GET", &format!("/api/v1/files/{id}"), None)?;
    if matches!(a.status, 403 | 404 | 410) {
        metas().lock().unwrap_or_else(|p| p.into_inner()).remove(&key);
        return Err(file_not_found());
    }
    let v = ok_or_refuse(a)?;
    let att = if v["attachment"].is_object() { v["attachment"].clone() } else { v };
    let mut m = metas().lock().unwrap_or_else(|p| p.into_inner());
    if m.len() >= META_MAX {
        m.retain(|_, (at, _)| at.elapsed() < META_TTL);
        if m.len() >= META_MAX {
            m.clear();
        }
    }
    m.insert(key, (Instant::now(), att.clone()));
    Ok(att)
}

/// The request headers a file fetch passes to Sew.
pub fn forwarded_headers(head: &str) -> Vec<(String, String)> {
    let head = crate::routes::http::request_head(head);
    ["range", "if-range", "if-none-match", "if-modified-since"]
        .iter()
        .filter_map(|n| crate::routes::http::extract_header(head, n).map(|v| (n.to_string(), v.to_string())))
        .collect()
}

/// A file's bytes, ready to stream: the status and K2's response headers.
pub struct Fetched {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Box<dyn std::io::Read + Send>,
}

/// `GET file`: the caller's login, Home chat on, then [`fetch`].
pub fn open(caller: R<Caller>, p: &HashMap<String, String>, fwd: &[(String, String)]) -> R<Fetched> {
    let c = caller?;
    let (on, reason) = super::status();
    if !on {
        return Err(Refusal::new(503, "chat_unavailable", format!("chat isn't available on this server ({})", reason.unwrap_or("unknown"))));
    }
    fetch(&c, p, fwd)
}

/// A file's variant as person `c` may read it, from Sew's loopback.
pub fn fetch(c: &Caller, p: &HashMap<String, String>, fwd: &[(String, String)]) -> R<Fetched> {
    let id = sew_id(param(p, "id").ok_or_else(|| usage("name the file: id"))?, "id")?;
    let variant = param(p, "v").unwrap_or("original");
    let field = URL_FIELDS
        .iter()
        .find(|(_, v)| *v == variant)
        .map(|(f, _)| *f)
        .ok_or_else(|| usage("v is original, thumb, preview or poster"))?;
    let dl = param(p, "dl") == Some("1");
    // `range=a-b` in the query is a `Range: bytes=a-b` (a browser's ranged
    // read without a header of its own); a real header wins.
    let mut fwd: Vec<(String, String)> = fwd.to_vec();
    if let Some(r) = param(p, "range") {
        let ok = r.split_once('-').is_some_and(|(a, b)| !a.is_empty() && a.bytes().all(|c| c.is_ascii_digit()) && b.bytes().all(|c| c.is_ascii_digit()));
        if !ok {
            return Err(usage("range is <first>-<last> (bytes)"));
        }
        if !fwd.iter().any(|(k, _)| k == "range") {
            fwd.push(("range".to_string(), format!("bytes={r}")));
        }
    }
    let fwd = fwd.as_slice();
    for fresh in [false, true] {
        let att = meta(c, &id, fresh)?;
        let Some(url) = att[field].as_str().filter(|s| !s.is_empty()) else {
            return Err(Refusal::new(404, "no_variant", format!("this file has no {variant}")));
        };
        let path = signed_path(url).ok_or_else(|| unavailable("Sew's file URL isn't Sew's API"))?;
        let r: SewBytes = deps().sew.fetch(&path, fwd).map_err(unavailable)?;
        // A URL past its time, or a file gone since it was kept: ask again.
        if matches!(r.status, 401 | 403 | 404 | 410) && !fresh {
            metas().lock().unwrap_or_else(|p| p.into_inner()).remove(&(c.ext.clone(), id.clone()));
            continue;
        }
        if matches!(r.status, 401 | 403 | 404 | 410) {
            return Err(file_not_found());
        }
        if !((200..300).contains(&r.status) || r.status == 304 || r.status == 416) {
            return Err(Refusal::new(502, "sew_error", format!("Sew answered {} for the file", r.status)));
        }
        let headers = served_headers(&r.headers, &att, variant, dl);
        return Ok(Fetched { status: r.status, headers, body: r.body });
    }
    Err(file_not_found())
}

/// A type that must never reach a browser as itself.
fn risky_type(t: &str) -> bool {
    ["html", "xml", "svg", "javascript", "ecmascript", "xhtml"].iter().any(|x| t.contains(x)) || t.starts_with("text/")
}

/// `filename="…"` (ASCII, quotes and controls out) and `filename*` (UTF-8).
pub fn disposition(kind: &str, name: &str) -> String {
    let ascii: String = name.chars().map(|ch| if (ch.is_ascii_graphic() && ch != '"' && ch != '\\') || ch == ' ' { ch } else { '_' }).collect();
    let mut enc = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            enc.push(b as char);
        } else {
            enc.push_str(&format!("%{b:02X}"));
        }
    }
    let ascii = if ascii.trim().is_empty() { "file".to_string() } else { ascii };
    format!("{kind}; filename=\"{ascii}\"; filename*=UTF-8''{enc}")
}

/// K2's headers for a file: Sew's length, range and validators; the type
/// (risky ones as octet-stream); attachment unless it is a safe image;
/// nosniff and the sandbox CSP always.
pub fn served_headers(sew: &[(String, String)], att: &Value, variant: &str, dl: bool) -> Vec<(String, String)> {
    let get = |n: &str| sew.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
    let mut out = Vec::new();
    for n in ["content-length", "content-range", "accept-ranges", "etag", "last-modified"] {
        if let Some(v) = get(n) {
            out.push((n.to_string(), v));
        }
    }
    let ct = get("content-type").unwrap_or_else(|| "application/octet-stream".to_string());
    let base = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let ct = if risky_type(&base) || base.is_empty() { "application/octet-stream".to_string() } else { ct };
    let inline = !dl && (variant != "original" || SAFE_INLINE.contains(&base.as_str()));
    let name = att["filename"].as_str().filter(|s| !s.is_empty()).unwrap_or("file");
    out.push(("content-type".into(), ct));
    out.push(("content-disposition".into(), disposition(if inline { "inline" } else { "attachment" }, name)));
    out.push(("x-content-type-options".into(), "nosniff".into()));
    out.push(("content-security-policy".into(), FILE_CSP.into()));
    // Sew's renders never change for an id; an original's bytes don't
    // either, but its access can, so it is kept briefly.
    let cache = if variant == "original" { "private, max-age=300".to_string() } else { format!("private, max-age={RENDER_MAX_AGE_SECS}, immutable") };
    out.push(("cache-control".into(), cache));
    out
}

fn status_text(s: u16) -> &'static str {
    match s {
        200 => "200 OK",
        206 => "206 Partial Content",
        304 => "304 Not Modified",
        416 => "416 Range Not Satisfiable",
        s if (200..300).contains(&s) => "200 OK",
        _ => "502 Bad Gateway",
    }
}

/// The response head for `f` (no line break can come from a value).
pub fn head_of(f: &Fetched) -> String {
    let mut h = format!("HTTP/1.1 {}\r\n", status_text(f.status));
    for (k, v) in &f.headers {
        if v.chars().any(|c| c == '\r' || c == '\n') {
            continue;
        }
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    h.push_str("Access-Control-Allow-Origin: *\r\nAccess-Control-Expose-Headers: *\r\n");
    h.push_str(k2_core::frame_policy::CONNECT_HEADER_LINES);
    h.push_str("Connection: close\r\n\r\n");
    h
}

/// Stream `f` to the client: the head, then the body as Sew sends it (a
/// reader thread, so a slow client paces Sew and a gone one stops it).
pub async fn stream_out(stream: &mut tokio::net::TcpStream, f: Fetched) {
    let head = head_of(&f);
    crate::slow_request::note_status(status_text(f.status));
    crate::routes::answer_probe::before_answer(stream).await;
    if stream.write_all(head.as_bytes()).await.is_err() || f.status == 304 {
        return;
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
    let mut body = f.body;
    let reader = tokio::task::spawn_blocking(move || {
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            match body.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    while let Some(chunk) = rx.recv().await {
        if stream.write_all(&chunk).await.is_err() {
            break;
        }
    }
    drop(rx);
    let _ = reader.await;
    let _ = stream.flush().await;
}

// ── files in a push (contract-home-chat-attachments-v1, the push note) ──

/// What a message's files say in a phone push: a short line ("Photo",
/// "2 photos", "Video", "File: report.pdf") and `attachmentCount` with the
/// first file's `kind`, so a sealed push says it too. Never a URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesSummary {
    pub count: u32,
    /// The first file's kind (`image|video|audio|document|archive|other`).
    pub kind: String,
    /// The first file's name.
    pub name: String,
    pub all_images: bool,
    pub all_videos: bool,
}

/// The longest file name a push line carries.
const PUSH_NAME_MAX: usize = 60;

impl FilesSummary {
    /// From Sew's `attachments[]` (`None`: no files).
    pub fn of(atts: &Value) -> Option<FilesSummary> {
        let list = atts.as_array().filter(|l| !l.is_empty())?;
        let kind_of = |a: &Value| a["kind"].as_str().unwrap_or("other").to_string();
        let first = &list[0];
        let name: String = first["filename"].as_str().unwrap_or("file").chars().filter(|c| !c.is_control()).take(PUSH_NAME_MAX).collect();
        Some(FilesSummary {
            count: u32::try_from(list.len()).unwrap_or(u32::MAX),
            kind: kind_of(first),
            name: if name.trim().is_empty() { "file".to_string() } else { name },
            all_images: list.iter().all(|a| kind_of(a) == "image"),
            all_videos: list.iter().all(|a| kind_of(a) == "video"),
        })
    }

    /// The push line for a message with no text of its own.
    pub fn line(&self) -> String {
        match (self.count, self.all_images, self.all_videos) {
            (1, true, _) => "Photo".to_string(),
            (n, true, _) => format!("{n} photos"),
            (1, _, true) => "Video".to_string(),
            (n, _, true) => format!("{n} videos"),
            (1, _, _) => format!("File: {}", self.name),
            (n, _, _) => format!("{n} files"),
        }
    }
}

type MemoKey = (String, String);

struct Memo {
    map: HashMap<MemoKey, FilesSummary>,
    order: std::collections::VecDeque<MemoKey>,
}

const MEMO_MAX: usize = 2048;

fn memo() -> &'static Mutex<Memo> {
    static M: OnceLock<Mutex<Memo>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(Memo { map: HashMap::new(), order: std::collections::VecDeque::new() }))
}

/// The person's gateway saw message `m` (`message_created` / `_updated`):
/// keep what its files would say in a push, so the push path needn't ask.
pub fn note_message(ext: &str, m: &Value) {
    let Some(id) = id_str(&m["id"]) else { return };
    let Some(s) = FilesSummary::of(&m["attachments"]) else { return };
    let key = (ext.to_string(), id);
    let mut g = memo().lock().unwrap_or_else(|p| p.into_inner());
    if g.map.insert(key.clone(), s).is_none() {
        g.order.push_back(key);
    }
    while g.order.len() > MEMO_MAX {
        if let Some(k) = g.order.pop_front() {
            g.map.remove(&k);
        }
    }
}

/// What message `msg`'s files say, for person `ext`: what their gateway
/// saw, else (when `ask`) one read of that message as them.
pub fn message_files(ext: &str, conv: &str, msg: &str, ask: bool) -> Option<FilesSummary> {
    if let Some(s) = memo().lock().unwrap_or_else(|p| p.into_inner()).map.get(&(ext.to_string(), msg.to_string())) {
        return Some(s.clone());
    }
    if !ask || !k2_core::tool_hook::sew_wire::is_sew_id(conv) {
        return None;
    }
    let after: u64 = msg.parse().ok()?;
    let path = format!("/api/v1/channels/{conv}/messages?before={}&limit=1", after.checked_add(1)?);
    let a = sew_as(ext, "GET", &path, None).ok()?;
    if !a.ok() {
        return None;
    }
    let items = a.body.get("items").unwrap_or(&a.body);
    let m = items.as_array()?.iter().find(|m| id_str(&m["id"]).as_deref() == Some(msg))?;
    note_message(ext, m);
    FilesSummary::of(&m["attachments"])
}
