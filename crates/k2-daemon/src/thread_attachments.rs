//! Thread attachments, daemon side (0.46.1, `thread-attachments-v1`; eval
//! `.k2/prds/eval-thread-file-objects-v1.md` S1 + S2). The model and the
//! send-time checks are `k2_core::overlay::attachments`; this module is
//! what viewers get:
//!
//! - **Live state** on every read: each stored attachment gains `state`
//!   (`ok` / `changed` / `gone`) from a stat, never stored.
//! - **Guests** (App passes): an attachment is a pointer, never a grant.
//!   A guest sees it only when their `files:read` and R3 folder scope in
//!   that room already cover the path; otherwise it is
//!   `{kind:"file", state:"hidden"}`, with no name, path or size. Applied
//!   to `GET /cli/thread` (the page, and the thread-delta-v1 `after_seq`
//!   read's `items` and `changed`, through `overlay_routes::
//!   thread_items_view`) and to each guest's overlay WS frames.
//! - **The read door** `GET /cli/thread/attachment` (previews and ranged
//!   downloads for the phone): Thread read rights plus the viewer's own
//!   file rights. POST is 405 (central guard).
//! - **Thumbnails** (thread-images-v1, 0.46.3, S3): `mode=thumb&px=256|512`
//!   for every image type (PNG, JPEG, GIF, WebP, BMP, TIFF, HEIC/HEIF,
//!   SVG), made by `k2_core::overlay::thumbs` and cached. An SVG is never
//!   served raw: thumb and preview both answer the allowlist rewrite
//!   (`sanitized: true`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use k2_core::overlay::attachments::{self as att, ThreadAttachment};
use k2_core::skin::SkinPass;

use crate::cli::{opt_param, str_param};
use crate::cli_response::CliResponse;

/// `/boot-status` feature key. A client shows no chips without it.
pub const FEATURE: &str = "thread-attachments-v1";
/// `/boot-status` feature key for `mode=thumb` (inline images, S3).
pub const FEATURE_IMAGES: &str = "thread-images-v1";
/// Route path.
pub const ROUTE: &str = "/cli/thread/attachment";
/// `mode=preview` for text: the first this-many bytes.
pub const PREVIEW_TEXT_BYTES: u64 = 512 * 1024;
/// `mode=preview` for an image or a PDF: whole files up to this size.
pub const PREVIEW_BINARY_MAX: u64 = 20 * 1024 * 1024;
/// `mode=range`: the most one chunk carries (as `/cli/fs/read-range`).
pub const RANGE_MAX: u64 = 16 * 1024 * 1024;
/// `mode=range` with no `len`.
pub const RANGE_DEFAULT: u64 = 4 * 1024 * 1024;

/// A workspace's folder as registered.
pub(crate) fn project_root(project_id: &str) -> Option<PathBuf> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let p = k2_core::db::schema::Project::get(&conn, project_id).ok()?;
    let path = p.path.trim().to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Who is looking. A guest is checked per attachment; everyone else
/// (owner token, Connect logins, a same-workspace agent) sees them all.
pub(crate) enum Viewer<'a> {
    Full,
    Guest { pass: &'a SkinPass, project_id: &'a str },
}

fn guest_sees(pass: &SkinPass, project_id: &str, root: &Path, a: &ThreadAttachment) -> bool {
    crate::fs_routes::guest_may_read_rel(pass, project_id, &root.to_string_lossy(), &a.path)
}

/// May an App guest (or App pass) attach these files to a post in
/// `project_id`'s Thread? Each must be named room-relative (no absolute,
/// `~` or `..` path) and sit under its own `files:read` folder scope.
/// Err = the uniform `files:read` missing-cap 403 (no existence oracle).
pub(crate) fn guest_may_attach(
    pass: &SkinPass,
    project_id: &str,
    root: &Path,
    reqs: &[k2_core::overlay::attachments::AttachmentRequest],
) -> Result<(), CliResponse> {
    let root_s = root.to_string_lossy();
    for r in reqs {
        let rel = r.path.trim();
        let relative = !rel.is_empty()
            && !rel.starts_with('/')
            && !rel.starts_with('~')
            && !rel.contains('\\')
            && Path::new(rel).components().all(|c| matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir));
        if !relative || !crate::fs_routes::guest_may_read_rel(pass, project_id, &root_s, rel) {
            return Err(cap_refused(crate::skin_routes::FILES_READ));
        }
    }
    Ok(())
}

/// The attachments of one doc as `viewer` sees them, with live state.
/// `live`: stat each file (GET); else every visible one is `ok` (a frame
/// sent at post time).
pub(crate) fn view_list(root: Option<&Path>, atts: &[ThreadAttachment], viewer: &Viewer<'_>, live: bool) -> Vec<serde_json::Value> {
    atts.iter()
        .map(|a| {
            if let Viewer::Guest { pass, project_id } = viewer {
                match root {
                    Some(r) if guest_sees(pass, project_id, r, a) => {}
                    _ => return att::hidden_json(),
                }
            }
            let state = match (live, root) {
                (false, _) => att::STATE_OK,
                (true, Some(r)) => att::live_state(r, a, false),
                (true, None) => att::STATE_GONE,
            };
            att::view_json(a, state)
        })
        .collect()
}

/// Rewrite `doc.attachments` in a serialized doc (`doc` is the doc JSON)
/// for `viewer`. No-op when the doc has none.
pub(crate) fn rewrite_doc(doc: &mut serde_json::Value, root: Option<&Path>, viewer: &Viewer<'_>, live: bool) {
    let Some(raw) = doc.get("attachments") else { return };
    let Ok(atts) = serde_json::from_value::<Vec<ThreadAttachment>>(raw.clone()) else {
        // Never pass through what we can't read as attachments.
        if let Some(o) = doc.as_object_mut() {
            o.remove("attachments");
        }
        return;
    };
    if atts.is_empty() {
        return;
    }
    let views = view_list(root, &atts, viewer, live);
    doc["attachments"] = serde_json::Value::Array(views);
}

/// `GET /cli/thread` page items (`[{…, doc}]`): live state, guest strip.
pub(crate) fn rewrite_items(items: &mut serde_json::Value, project_id: &str, pass: Option<&SkinPass>) {
    let Some(arr) = items.as_array_mut() else { return };
    if !arr.iter().any(|it| it["doc"]["attachments"].as_array().is_some_and(|a| !a.is_empty())) {
        return;
    }
    let root = project_root(project_id);
    let viewer = match pass {
        Some(p) => Viewer::Guest { pass: p, project_id },
        None => Viewer::Full,
    };
    for it in arr.iter_mut() {
        if let Some(doc) = it.get_mut("doc") {
            rewrite_doc(doc, root.as_deref(), &viewer, true);
        }
    }
}

fn error_json(status: &'static str, code: &str, hint: impl std::fmt::Display) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "ok": false, "error": { "code": code, "hint": hint.to_string() } }).to_string(),
    }
}

/// This route's missing-permission refusal: a stable code (the App
/// surfaces' `cap_not_granted`) so a phone never string-matches. The same
/// body whether or not the file exists (no existence oracle).
pub(crate) fn cap_refused(cap: &str) -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": "cap_not_granted", "cap": cap, "hint": format!("missing capability {cap}") },
        })
        .to_string(),
    }
}

fn gone() -> CliResponse {
    error_json("404 Not Found", "attachment_gone", "the file is no longer there (moved or deleted)")
}

/// One audit line (`thread.attachment.*`). Never file contents.
pub(crate) fn audit(event: &str, who: &str, detail: String) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(event, who, detail, "-", "-", "cli"));
}

/// The resolved Thread the route reads (`overlay_routes` resolves and
/// checks Thread rights).
pub(crate) struct ResolvedThread {
    pub conversation_id: String,
    pub project_id: String,
}

/// `GET /cli/thread/attachment?addr|conversationId=&id=&n=&mode=meta|preview|range[&offset=&len=]`.
/// `resolved`: the Thread, already resolved for this caller with Thread
/// read rights checked (a guest: room, pinned Chat, `thread:read`).
/// `pass`: the guest's pass, `None` for the owner and Connect logins.
pub(crate) fn handle_get(params: &HashMap<String, String>, resolved: &ResolvedThread, pass: Option<&SkinPass>) -> CliResponse {
    let id = str_param(params, "id");
    if id.trim().is_empty() {
        return error_json("400 Bad Request", "usage", "missing id (the message's doc id)");
    }
    let n: usize = match opt_param(params, "n").unwrap_or_else(|| "0".into()).trim().parse() {
        Ok(n) => n,
        Err(_) => return error_json("400 Bad Request", "usage", "n must be the attachment's index (0-based)"),
    };
    let mode = opt_param(params, "mode").unwrap_or_else(|| "meta".into());
    if !matches!(mode.as_str(), "meta" | "preview" | "range" | "thumb") {
        return error_json("400 Bad Request", "usage", "mode is meta, preview, thumb or range");
    }
    let px = match k2_core::overlay::thumbs::parse_px(opt_param(params, "px").as_deref()) {
        Ok(px) => px,
        Err(e) => return error_json("400 Bad Request", "usage", e),
    };
    let doc = match k2_core::overlay::get_thread_doc(&resolved.conversation_id, id.trim()) {
        Ok(Some(d)) => d,
        Ok(None) => return error_json("404 Not Found", "not_found", "no such message in this Thread"),
        Err(e) => return error_json("500 Internal Server Error", "store", e),
    };
    let Some(a) = doc.attachments.get(n) else {
        return error_json("404 Not Found", "not_found", "no such attachment on this message");
    };
    let Some(root) = project_root(&resolved.project_id) else {
        return gone();
    };
    // A pointer, not a grant: a guest opens only what their own file
    // rights already cover. `meta` answers what `GET /cli/thread` shows
    // (`hidden`); preview and range are the uniform `cap_not_granted` 403.
    if let Some(p) = pass {
        if !guest_sees(p, &resolved.project_id, &root, a) {
            if mode == "meta" {
                return CliResponse::ok_json(
                    serde_json::json!({ "ok": true, "kind": att::KIND_FILE, "state": att::STATE_HIDDEN }).to_string(),
                );
            }
            return cap_refused(crate::skin_routes::FILES_READ);
        }
    }
    let Some(real) = att::resolve_now(&root, a) else {
        return gone();
    };
    let Ok(meta) = std::fs::metadata(&real) else {
        return gone();
    };
    let who = match pass {
        Some(p) => Some(format!("guest:{}", p.username.trim())),
        None => crate::read_routes::request_login().filter(|l| l != "owner").map(|l| format!("user:{l}")),
    };
    if let Some(who) = who.as_deref() {
        audit(
            "thread.attachment.open",
            who,
            format!("conversation={} doc={} rel={:?} mode={mode}", resolved.conversation_id, doc.id, a.path),
        );
    }
    let size = meta.len();
    match mode.as_str() {
        "meta" => {
            let mut v = serde_json::json!({
                "ok": true,
                "kind": a.kind,
                "name": a.name,
                "path": a.path,
                "size": size,
                "mime": a.mime,
                "modifiedAt": att::modified_ms(&meta),
                "sha256": a.sha256,
                "state": att::live_state(&root, a, true),
                "sent": { "size": a.size, "modifiedAt": a.modified_at },
            });
            if let Some(l) = a.line {
                v["line"] = serde_json::json!(l);
            }
            if let Some(l) = a.line_end {
                v["lineEnd"] = serde_json::json!(l);
            }
            CliResponse::ok_json(v.to_string())
        }
        "preview" => preview(a, &real, size, att::live_state(&root, a, true)),
        "thumb" => thumb(&resolved.project_id, a, &real, &meta, px),
        _ => range(params, &real),
    }
}

fn preview_kind(mime: &str) -> Option<&'static str> {
    if k2_core::overlay::thumbs::is_image_mime(mime) {
        Some("image")
    } else if mime == "application/pdf" {
        Some("pdf")
    } else if mime.starts_with("text/") || matches!(mime, "application/json" | "application/xml") {
        Some("text")
    } else {
        None
    }
}

fn preview(a: &ThreadAttachment, real: &Path, size: u64, state: &str) -> CliResponse {
    match preview_kind(&a.mime) {
        Some("text") => {
            let want = size.min(PREVIEW_TEXT_BYTES) as usize;
            let bytes = match read_prefix(real, want) {
                Ok(b) => b,
                Err(e) => return error_json("500 Internal Server Error", "io", e),
            };
            if bytes[..bytes.len().min(8192)].contains(&0u8) {
                return error_json("415 Unsupported Media Type", "no_preview", "this file has no text preview; download it");
            }
            // Never cut a UTF-8 sequence in half at the 512 KB edge.
            let text = match std::str::from_utf8(&bytes) {
                Ok(s) => s.to_string(),
                Err(e) if e.error_len().is_none() => String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned(),
                Err(_) => String::from_utf8_lossy(&bytes).into_owned(),
            };
            CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "kind": "text",
                    "mime": a.mime,
                    "name": a.name,
                    "size": size,
                    "state": state,
                    "text": k2_core::redaction::redact(&text),
                    "truncated": size > PREVIEW_TEXT_BYTES,
                })
                .to_string(),
            )
        }
        Some("image") if a.mime == "image/svg+xml" => {
            // Never the raw SVG: the same allowlist rewrite as a thumb.
            match k2_core::overlay::thumbs::make(real, &a.mime, k2_core::overlay::thumbs::PX_LARGE) {
                Ok(t) => CliResponse::ok_json(
                    serde_json::json!({
                        "ok": true,
                        "kind": "image",
                        "mime": t.mime,
                        "name": a.name,
                        "size": size,
                        "state": state,
                        "sanitized": true,
                        "width": t.width,
                        "height": t.height,
                        "base64": base64::engine::general_purpose::STANDARD.encode(&t.bytes),
                    })
                    .to_string(),
                ),
                Err(e) => thumb_error(e),
            }
        }
        Some(kind) => {
            if size > PREVIEW_BINARY_MAX {
                return error_json(
                    "413 Payload Too Large",
                    "preview_too_large",
                    format!("previews stop at {} MB; download it with mode=range", PREVIEW_BINARY_MAX / (1024 * 1024)),
                );
            }
            match std::fs::read(real) {
                Ok(bytes) => CliResponse::ok_json(
                    serde_json::json!({
                        "ok": true,
                        "kind": kind,
                        "mime": a.mime,
                        "name": a.name,
                        "size": size,
                        "state": state,
                        "base64": base64::engine::general_purpose::STANDARD.encode(&bytes),
                    })
                    .to_string(),
                ),
                Err(e) => error_json("500 Internal Server Error", "io", e),
            }
        }
        None => error_json("415 Unsupported Media Type", "no_preview", "this file type has no preview; download it"),
    }
}

fn thumb_error(e: k2_core::overlay::thumbs::ThumbError) -> CliResponse {
    use k2_core::overlay::thumbs::ThumbError;
    let status = match &e {
        ThumbError::Unsupported(_) | ThumbError::Bad(_) => "415 Unsupported Media Type",
        ThumbError::TooLarge(_) => "413 Payload Too Large",
        ThumbError::Io(_) => "500 Internal Server Error",
    };
    error_json(status, e.code(), e.hint())
}

/// `mode=thumb` (thread-images-v1): an image attachment's thumbnail, from
/// the cache when the file is unchanged since it was made.
fn thumb(project_id: &str, a: &ThreadAttachment, real: &Path, meta: &std::fs::Metadata, px: u32) -> CliResponse {
    use k2_core::overlay::thumbs;
    if !thumbs::is_image_mime(&a.mime) {
        return error_json("415 Unsupported Media Type", "no_thumbnail", "this file isn't an image; open it or download it");
    }
    let key = thumbs::cache_key(project_id, &a.path, meta.len(), att::modified_ms(meta), px);
    let dir = thumbs::cache_dir();
    let t = match thumbs::cache_get(&dir, &key) {
        Some(t) => t,
        None => match thumbs::make(real, &a.mime, px) {
            Ok(t) => {
                thumbs::cache_put(&dir, &key, &t, thumbs::CACHE_MAX_BYTES);
                t
            }
            Err(e) => return thumb_error(e),
        },
    };
    let unchanged = meta.len() == a.size && att::modified_ms(meta) == a.modified_at;
    let mut v = serde_json::json!({
        "ok": true,
        "kind": "image",
        "mime": t.mime,
        "name": a.name,
        "size": meta.len(),
        "state": if unchanged { att::STATE_OK } else { att::STATE_CHANGED },
        "width": t.width,
        "height": t.height,
        "px": px,
        "base64": base64::engine::general_purpose::STANDARD.encode(&t.bytes),
    });
    if t.sanitized {
        v["sanitized"] = serde_json::json!(true);
    }
    CliResponse::ok_json(v.to_string())
}

fn read_prefix(path: &Path, n: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buf = Vec::with_capacity(n);
    f.take(n as u64).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

fn range(params: &HashMap<String, String>, real: &Path) -> CliResponse {
    let offset: u64 = match opt_param(params, "offset").map(|v| v.trim().parse()) {
        Some(Ok(v)) => v,
        Some(Err(_)) => return error_json("400 Bad Request", "usage", "offset must be a byte offset"),
        None => 0,
    };
    let len: u64 = match opt_param(params, "len").map(|v| v.trim().parse()) {
        Some(Ok(v)) => v,
        Some(Err(_)) => return error_json("400 Bad Request", "usage", "len must be a byte count"),
        None => RANGE_DEFAULT,
    };
    if len == 0 || len > RANGE_MAX {
        return error_json("400 Bad Request", "usage", format!("len is 1..={RANGE_MAX}"));
    }
    match k2_core::fs_commands::read_file_range(&real.to_string_lossy(), offset, len) {
        Ok((bytes, size)) => {
            let eof = offset.saturating_add(bytes.len() as u64) >= size;
            CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "base64": base64::engine::general_purpose::STANDARD.encode(&bytes),
                    "len": bytes.len() as u64,
                    "offset": offset,
                    "size": size,
                    "eof": eof,
                })
                .to_string(),
            )
        }
        Err(e) => error_json("400 Bad Request", "usage", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_kinds() {
        assert_eq!(preview_kind("image/png"), Some("image"));
        for m in ["image/bmp", "image/heif", "image/heic", "image/tiff", "image/svg+xml", "image/gif", "image/webp"] {
            assert_eq!(preview_kind(m), Some("image"), "{m}");
        }
        assert_eq!(preview_kind("application/pdf"), Some("pdf"));
        assert_eq!(preview_kind("text/markdown"), Some("text"));
        assert_eq!(preview_kind("text/plain"), Some("text"));
        assert_eq!(preview_kind("application/json"), Some("text"));
        assert_eq!(preview_kind("application/zip"), None);
        assert_eq!(preview_kind("application/octet-stream"), None);
    }

    #[test]
    fn unreadable_attachments_are_dropped_not_passed_through() {
        let mut doc = serde_json::json!({ "id": "x", "attachments": [{ "kind": "file", "path": 3 }] });
        rewrite_doc(&mut doc, None, &Viewer::Full, true);
        assert!(doc.get("attachments").is_none(), "{doc}");
    }

    #[test]
    fn a_missing_root_reads_as_gone() {
        let a = ThreadAttachment {
            kind: "file".into(),
            path: "a.md".into(),
            name: "a.md".into(),
            size: 1,
            mime: "text/markdown".into(),
            modified_at: 1,
            sha256: None,
            line: Some(3),
            line_end: None,
        };
        let v = view_list(None, std::slice::from_ref(&a), &Viewer::Full, true);
        assert_eq!(v[0]["state"], "gone");
        assert_eq!(v[0]["line"], 3);
        let v = view_list(None, std::slice::from_ref(&a), &Viewer::Full, false);
        assert_eq!(v[0]["state"], "ok", "a post-time frame says ok");
    }
}
