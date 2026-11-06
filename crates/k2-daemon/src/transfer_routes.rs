//! The pass side of resumable transfers (0.46.9, `k2_core::fs_transfer`;
//! `k2 study app-transfers`). An App's server (gateway link) or the
//! browser helper calls these as the person; the bytes themselves go to
//! the door (`transfer_door`) with the token these answer.
//!
//! ```text
//! POST /cli/fs/upload/create    files.uploadCreate    (fs_routes::handle_skin_upload_create)
//! GET  /cli/fs/upload/status    files.uploadStatus    ?uploadId=
//! POST /cli/fs/upload/complete  files.uploadComplete  {uploadId, sha256?}
//! POST /cli/fs/upload/abort     files.uploadAbort     {uploadId}
//! POST /cli/fs/upload/token     files.uploadToken     {uploadId, tokenTtlSecs?}
//! POST /cli/fs/download/token   files.downloadToken   {workspace, relPath, tokenTtlSecs?}
//! POST /cli/thread/attachment/token  thread.attachmentToken  {addr, id, n, tokenTtlSecs?}
//! ```
//!
//! An App guest's pass only (a session pass or a platform pass); the owner
//! token and Connect logins get a usage 400, as on `files.download`
//! (people use the attachment token, which admits them). A session is its
//! maker's: another person's upload id is the same 404 as one that never
//! existed.

use std::collections::HashMap;

use k2_core::fs_transfer::{self as ft, TransferError};
use k2_core::skin::SkinPass;
use serde_json::{json, Value};

use crate::cli_response::CliResponse;

pub const ROUTE_CREATE: &str = "/cli/fs/upload/create";
pub const ROUTE_STATUS: &str = "/cli/fs/upload/status";
pub const ROUTE_COMPLETE: &str = "/cli/fs/upload/complete";
pub const ROUTE_ABORT: &str = "/cli/fs/upload/abort";
pub const ROUTE_TOKEN: &str = "/cli/fs/upload/token";
pub const ROUTE_DOWNLOAD_TOKEN: &str = "/cli/fs/download/token";
pub const ROUTE_ATTACHMENT_TOKEN: &str = "/cli/thread/attachment/token";

/// `/boot-status` feature key: the resumable transfer routes and the door.
pub const FEATURE: &str = "transfers-v1";

/// The upload routes a guest's pass reaches (create and the rest).
pub fn is_upload_route(p: &str) -> bool {
    matches!(p, ROUTE_CREATE | ROUTE_STATUS | ROUTE_COMPLETE | ROUTE_ABORT | ROUTE_TOKEN)
}

/// `{error: {code, hint}}` plus `extra`'s fields.
pub fn error_json(status: &'static str, code: &str, hint: impl std::fmt::Display, extra: Option<Value>) -> CliResponse {
    let mut v = json!({ "error": { "code": code, "hint": hint.to_string() } });
    if let Some(Value::Object(m)) = extra {
        for (k, x) in m {
            v[k] = x;
        }
    }
    CliResponse { status, content_type: "application/json", body: v.to_string() }
}

/// Status, code and extra fields for one refusal. `door`: the token door
/// (an unknown id or a bad token is 401 there; on the pass side it is a
/// plain 404, no oracle about other people's uploads).
pub fn refusal(e: &TransferError, door: bool) -> (&'static str, &'static str, Option<Value>) {
    use TransferError as E;
    match e {
        E::Unauthorized if door => ("401 Unauthorized", "transfer_token_invalid", None),
        E::Unauthorized => ("404 Not Found", "not_found", None),
        E::TokenExpired => ("401 Unauthorized", "transfer_token_expired", None),
        E::Expired => ("410 Gone", "transfer_expired", None),
        E::AlreadyDone => ("409 Conflict", "upload_done", None),
        E::BadRange(_) | E::TooFragmented => ("400 Bad Request", "bad_range", None),
        E::TooLarge(_) => ("413 Payload Too Large", "too_large", None),
        E::RangeChecksum { expected, actual } => {
            ("422 Unprocessable Entity", "checksum_mismatch", Some(json!({ "range": true, "expected": expected, "actual": actual })))
        }
        E::Incomplete { missing } => {
            ("409 Conflict", "upload_incomplete", Some(json!({ "missing": ft::RangeSet::inclusive(missing) })))
        }
        E::ChecksumMismatch { expected, actual } => {
            ("422 Unprocessable Entity", "checksum_mismatch", Some(json!({ "range": false, "expected": expected, "actual": actual })))
        }
        E::NoSpace(_) => ("507 Insufficient Storage", "no_space", None),
        E::TooMany(_) => ("429 Too Many Requests", "too_many_transfers", None),
        E::Invalid(_) => ("400 Bad Request", "usage", None),
        E::Io(_) => ("500 Internal Server Error", "failed", None),
    }
}

/// The pass side's answer for a refusal.
pub fn error_response(e: &TransferError) -> CliResponse {
    let (status, code, extra) = refusal(e, false);
    let hint = match e {
        TransferError::Unauthorized => "no such upload of yours".to_string(),
        other => other.to_string(),
    };
    error_json(status, code, hint, extra)
}

/// 400 for a pass K2 can't read again by its id (the App's own link pass).
pub fn guest_pass_required() -> CliResponse {
    error_json(
        "400 Bad Request",
        "guest_pass_required",
        "a resumable transfer is the person's: call it as them (k2.asGuest(pass)), not with the App's own pass",
        None,
    )
}

/// The owner token and Connect logins: these are an App guest's doors.
pub fn not_a_guest() -> CliResponse {
    error_json(
        "400 Bad Request",
        "usage",
        "resumable uploads and download tokens are an App guest's (its own pass); a person on this box uses the files routes",
        None,
    )
}

fn holder_principal(pass: &SkinPass) -> String {
    crate::fs_routes::guest_upload_principal(pass)
}

fn upload_id(v: &HashMap<String, String>) -> Result<String, CliResponse> {
    let id = v.get("uploadId").map(|s| s.trim().to_string()).unwrap_or_default();
    if id.is_empty() {
        return Err(error_json("400 Bad Request", "usage", "missing uploadId", None));
    }
    Ok(id)
}

fn ttl_param(v: &HashMap<String, String>, default: u64) -> Result<u64, CliResponse> {
    match v.get("tokenTtlSecs").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        None => Ok(ft::clamp_ttl(None, default)),
        Some(s) => match s.parse::<u64>() {
            Ok(n) => Ok(ft::clamp_ttl(Some(n), default)),
            Err(_) => Err(error_json("400 Bad Request", "usage", "tokenTtlSecs is a number of seconds", None)),
        },
    }
}

/// A JSON body as string params (numbers and booleans as text).
pub fn body_params(body: &[u8]) -> Result<HashMap<String, String>, CliResponse> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(HashMap::new());
    }
    let v: Value = serde_json::from_slice(body)
        .map_err(|e| error_json("400 Bad Request", "usage", format!("invalid JSON body: {e}"), None))?;
    let Value::Object(m) = v else {
        return Err(error_json("400 Bad Request", "usage", "the body is a JSON object", None));
    };
    Ok(m.into_iter()
        .filter_map(|(k, x)| match x {
            Value::String(s) => Some((k, s)),
            Value::Number(n) => Some((k, n.to_string())),
            Value::Bool(b) => Some((k, b.to_string())),
            _ => None,
        })
        .collect())
}

/// The session this pass made, with its rights re-checked.
fn own_session(id: &str, pass: &SkinPass) -> Result<ft::UploadSession, CliResponse> {
    let s = ft::session_of(&ft::uploads_dir(), id, &holder_principal(pass), ft::now_secs()).map_err(|e| error_response(&e))?;
    if !crate::fs_routes::upload_still_allowed(pass, &s.project_id, &s.rel_dir, s.drop) {
        let cap = if s.drop { k2_core::skin::CAP_FILES_UPLOAD } else { crate::skin_routes::FILES_WRITE };
        return Err(crate::skin_routes::missing_cap_response(cap));
    }
    Ok(s)
}

/// `status`, `complete`, `abort`, `token` for an App guest. `params` holds
/// the query (GET) or the JSON body (POST).
pub fn handle(path: &str, params: &HashMap<String, String>, pass: &SkinPass) -> CliResponse {
    let id = match upload_id(params) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let store = ft::uploads_dir();
    let ledger = k2_core::fs_guest_upload::ledger_dir();
    let who = format!("guest:{}", pass.username.trim());
    match path {
        ROUTE_STATUS => match ft::session_of(&store, &id, &holder_principal(pass), ft::now_secs()) {
            Ok(s) => CliResponse::ok_json(s.status_json().to_string()),
            Err(e) => error_response(&e),
        },
        ROUTE_COMPLETE => {
            let sha = match ft::parse_sha256(params.get("sha256").map(String::as_str)) {
                Ok(s) => s,
                Err(e) => return error_json("400 Bad Request", "usage", e, None),
            };
            let s = match own_session(&id, pass) {
                Ok(s) => s,
                Err(r) => return r,
            };
            complete_answer(&store, &ledger, &s, sha.as_deref(), &who)
        }
        ROUTE_ABORT => {
            if let Err(e) = ft::session_of(&store, &id, &holder_principal(pass), ft::now_secs()) {
                if e != TransferError::Expired {
                    return error_response(&e);
                }
            }
            match ft::abort_upload(&store, &ledger, &id) {
                Ok(()) => {
                    crate::thread_attachments::audit("files.upload_abort", &who, format!("id={id}"));
                    CliResponse::ok_json(json!({ "ok": true, "uploadId": id, "aborted": true }).to_string())
                }
                Err(e) => error_response(&e),
            }
        }
        ROUTE_TOKEN => {
            let ttl = match ttl_param(params, ft::TOKEN_TTL_DEFAULT_SECS) {
                Ok(t) => t,
                Err(r) => return r,
            };
            if let Err(r) = own_session(&id, pass) {
                return r;
            }
            match ft::add_token(&store, &id, ttl, ft::now_secs()) {
                Ok((token, exp)) => CliResponse::ok_json(
                    json!({ "uploadId": id, "transferToken": token, "tokenExpiresAt": exp.saturating_mul(1000), "url": ft::upload_url(&id) })
                        .to_string(),
                ),
                Err(e) => error_response(&e),
            }
        }
        _ => CliResponse::not_found(),
    }
}

/// Complete `s` and answer (both doors): publish, tell the file watchers,
/// audit.
pub fn complete_answer(store: &std::path::Path, ledger: &std::path::Path, s: &ft::UploadSession, sha: Option<&str>, who: &str) -> CliResponse {
    let first = s.done.is_none();
    match ft::complete_upload(store, ledger, &s.id, sha, ft::now_secs()) {
        Ok(d) => {
            if first {
                crate::session_events::emit_fs_changed_for_paths([d.path.to_string_lossy().to_string()]);
                crate::thread_attachments::audit(
                    "files.upload_complete",
                    who,
                    format!("room={} rel={:?} size={} id={}", s.project_id, d.rel_path, d.size, s.id),
                );
            }
            CliResponse::ok_json(
                json!({ "uploadId": s.id, "done": true, "relPath": d.rel_path, "size": d.size, "sha256": d.sha256 }).to_string(),
            )
        }
        Err(e) => error_response(&e),
    }
}

/// The answer for a minted download grant (both token routes).
pub fn download_token_answer(g: &ft::DownloadGrant, token: &str, size: u64, etag: &str) -> CliResponse {
    crate::transfer_door::note_live();
    CliResponse::ok_json(
        json!({
            "ok": true,
            "transferToken": token,
            "url": ft::download_url(&g.id),
            "expiresAt": g.expires_at.saturating_mul(1000),
            "size": size,
            "etag": etag,
            "name": g.name,
            "mime": g.mime,
        })
        .to_string(),
    )
}

/// The token lifetime a download token route asks for.
pub fn download_ttl(params: &HashMap<String, String>) -> Result<u64, CliResponse> {
    ttl_param(params, ft::DOWNLOAD_TTL_DEFAULT_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusals_have_codes_and_no_oracle_on_the_pass_side() {
        assert_eq!(refusal(&TransferError::Unauthorized, false).1, "not_found");
        assert_eq!(refusal(&TransferError::Unauthorized, true).0, "401 Unauthorized");
        assert_eq!(refusal(&TransferError::Expired, true).0, "410 Gone");
        let (s, c, x) = refusal(&TransferError::Incomplete { missing: vec![(5, 10)] }, false);
        assert_eq!((s, c), ("409 Conflict", "upload_incomplete"));
        assert_eq!(x.expect("missing")["missing"], json!([[5, 9]]));
        let r = error_response(&TransferError::NoSpace("low".into()));
        assert_eq!(r.status, "507 Insufficient Storage");
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["code"], "no_space");
    }

    #[test]
    fn body_params_take_strings_numbers_and_booleans() {
        let p = body_params(br#"{"uploadId":"abc","tokenTtlSecs":120,"x":true,"y":{"z":1}}"#).expect("params");
        assert_eq!(p.get("uploadId").map(String::as_str), Some("abc"));
        assert_eq!(p.get("tokenTtlSecs").map(String::as_str), Some("120"));
        assert_eq!(p.get("x").map(String::as_str), Some("true"));
        assert!(!p.contains_key("y"));
        assert!(body_params(b"[1]").is_err());
        assert!(body_params(b"  ").expect("empty").is_empty());
        assert_eq!(ttl_param(&p, 600).expect("ttl"), 120);
    }
}
