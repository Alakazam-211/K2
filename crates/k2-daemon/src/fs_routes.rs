//! Daemon-side `/cli/fs/*` route handlers (Phase 2 Unit 6).
//!
//! Every handler is a thin wrapper around `k2_core::fs_commands::*`.
//! Bodies for POST routes are JSON; query-string for GET routes.
//!
//! Binary reads (`/cli/fs/read-binary`) base64-encode the file content
//! before placing it in the JSON response. This trades ~33% wire size
//! for transport-format simplicity (the existing `/cli/*` dispatcher
//! is JSON-only). Pre-Phase-3, this is acceptable up to the 50 MB
//! `read_binary` cap — the renderer's PDF/DOCX viewers don't open
//! anything larger.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::Deserialize;

use crate::cli_response::CliResponse;
use k2_core::fs_commands as fsc;
use k2_core::skin::SkinPass;

// ── GET handlers (query-string) ───────────────────────────────────────

pub fn handle_read_dir(params: &HashMap<String, String>) -> CliResponse {
    let path = match params.get("path") {
        Some(p) if !p.is_empty() => p.clone(),
        _ => return CliResponse::bad_request("Missing 'path' parameter"),
    };
    let show_hidden = matches!(
        params.get("show_hidden").map(|v| v.as_str()),
        Some("1") | Some("true") | Some("on")
    );
    match fsc::read_dir_coded(&path, show_hidden) {
        Ok(entries) => CliResponse::ok_json(
            serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string()),
        ),
        Err(e) => read_dir_refusal(e),
    }
}

/// `fs/read-dir`'s refusal: a folder that isn't there is a 404
/// `not_found` (0.46.6: it was a 400, which read as a bad request and
/// counted toward an App's refused-calls alert); anything else stays the
/// old 400. Both keep the old `{"error": "<text>"}` and add the `code`
/// (see `fsc::ReadFileError`).
fn read_dir_refusal(e: fsc::ReadFileError) -> CliResponse {
    if e.code == "not_found" {
        return read_dir_not_found();
    }
    CliResponse {
        status: "400 Bad Request",
        content_type: "application/json",
        body: serde_json::json!({ "error": e.message, "code": e.code }).to_string(),
    }
}

/// The 404 for a folder that isn't there. The text never names a host
/// path (an App's guest sees it too).
fn read_dir_not_found() -> CliResponse {
    CliResponse {
        status: "404 Not Found",
        content_type: "application/json",
        body: serde_json::json!({ "error": "No such folder", "code": "not_found" }).to_string(),
    }
}

/// 0.46.6: `rel` names nothing in the room, and the part of it that does
/// exist resolves inside the room. Returns the path it would have (for the
/// path-scope check). `None` when something is there (a dangling or
/// looping link, say) or the existing part leads out of the room: the
/// jail's own 400 stands.
fn missing_in_room(ws_root: &str, rel: &str) -> Option<PathBuf> {
    let rel = Path::new(rel.trim());
    if !rel.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir)) {
        return None;
    }
    let root = dunce::canonicalize(Path::new(ws_root)).ok()?;
    let target = root.join(rel);
    if std::fs::symlink_metadata(&target).is_ok() {
        return None;
    }
    let landed = resolve_existing_prefix(&target)?;
    landed.starts_with(&root).then_some(target)
}

pub fn handle_read_file(params: &HashMap<String, String>) -> CliResponse {
    let path = match params.get("path") {
        Some(p) if !p.is_empty() => p.clone(),
        _ => return CliResponse::bad_request("Missing 'path' parameter"),
    };
    match fsc::read_file_coded(&path) {
        Ok(content) => CliResponse::ok_json(
            serde_json::to_string(&content).unwrap_or_else(|_| "{}".to_string()),
        ),
        Err(e) => read_file_refusal(e),
    }
}

/// `fs/read-file`'s 400: the old `{"error": "<message>"}` plus the
/// refusal's `code` (`not_found`, `binary`, `not_utf8`, `too_large`,
/// `permission_denied`, `invalid_path`, `read_failed`; see
/// `fsc::ReadFileError`). The file viewer opens only `not_found` as a new,
/// editable file; any other code makes it read-only so an edit can't
/// overwrite a file it couldn't show.
fn read_file_refusal(e: fsc::ReadFileError) -> CliResponse {
    CliResponse {
        status: "400 Bad Request",
        content_type: "application/json",
        body: serde_json::json!({ "error": e.message, "code": e.code }).to_string(),
    }
}

/// Read a file as raw bytes and base64-encode for transport. The
/// renderer decodes via `atob(...)` or a Uint8Array constructor; both
/// PDF.js and the DOCX renderer take base64 directly.
pub fn handle_read_binary(params: &HashMap<String, String>) -> CliResponse {
    let path = match params.get("path") {
        Some(p) if !p.is_empty() => p.clone(),
        _ => return CliResponse::bad_request("Missing 'path' parameter"),
    };
    match fsc::read_binary_file(&path) {
        Ok(bytes) => {
            let b64 = B64.encode(&bytes);
            CliResponse::ok_json(serde_json::json!({ "base64": b64 }).to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// GET `/cli/fs/read-range?path=&offset=&len=` — one slice of a file,
/// base64 in JSON (the transport every `/cli/*` route shares). The
/// download counterpart of `fs/upload-chunk` (0.40.22): a client loops
/// ranged reads to stream a file of ANY size — per-request memory is
/// bounded by the 16 MB chunk cap, unlike `fs/read-binary`'s whole-file
/// 50 MB ceiling. Response: `{ base64, len, size, eof }`; offset-addressed
/// reads make a resume after disconnect free. Auth: `token_ok` via the
/// shared `/cli/` GET gate, same as every other fs read.
pub fn handle_read_range(params: &HashMap<String, String>) -> CliResponse {
    let path = match params.get("path") {
        Some(p) if !p.is_empty() => p.clone(),
        _ => return CliResponse::bad_request("Missing 'path' parameter"),
    };
    let offset: u64 = match params.get("offset").map(|v| v.parse()) {
        Some(Ok(v)) => v,
        Some(Err(_)) => return CliResponse::bad_request("invalid 'offset' parameter"),
        None => 0,
    };
    let len: u64 = match params.get("len").map(|v| v.parse()) {
        Some(Ok(v)) => v,
        Some(Err(_)) => return CliResponse::bad_request("invalid 'len' parameter"),
        None => 8 * 1024 * 1024,
    };
    match fsc::read_file_range(&path, offset, len) {
        Ok((bytes, size)) => {
            let eof = offset.saturating_add(bytes.len() as u64) >= size;
            CliResponse::ok_json(
                serde_json::json!({
                    "base64": B64.encode(&bytes),
                    "len": bytes.len() as u64,
                    "size": size,
                    "eof": eof,
                })
                .to_string(),
            )
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Report the daemon machine's filesystem basics so a remote client can
/// seed a folder browser at the host's home dir (instead of a hardcoded
/// `/`) and render paths with the host's separator. No path input — purely
/// describes the host. Gated like the other `fs/*` reads (`token_ok`).
pub fn handle_info(_params: &HashMap<String, String>) -> CliResponse {
    let home = k2_core::paths::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let separator = std::path::MAIN_SEPARATOR.to_string();
    let os = std::env::consts::OS.to_string();
    CliResponse::ok_json(
        serde_json::json!({
            "home": home,
            "separator": separator,
            "os": os,
        })
        .to_string(),
    )
}

pub fn handle_clipboard_paths(_params: &HashMap<String, String>) -> CliResponse {
    match fsc::clipboard_read_file_paths() {
        Ok(paths) => {
            CliResponse::ok_json(serde_json::to_string(&paths).unwrap_or_else(|_| "[]".to_string()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

// ── POST handlers (JSON bodies) ───────────────────────────────────────

#[derive(Deserialize)]
struct SearchTreeBody {
    root: String,
    query: String,
    #[serde(default)]
    show_hidden: bool,
    #[serde(default)]
    max_results: Option<usize>,
}

pub fn handle_search_tree(body: &[u8]) -> CliResponse {
    let parsed: SearchTreeBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let max = parsed.max_results.unwrap_or(500);
    match fsc::search_tree(&parsed.root, &parsed.query, parsed.show_hidden, max) {
        Ok(matches) => CliResponse::ok_json(
            serde_json::to_string(&matches).unwrap_or_else(|_| "[]".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct ResolveLinksBody {
    /// The message's workspace: a Thread address (`k2`, `k2/reviewer`),
    /// a registered workspace path, or a project id. The daemon resolves
    /// the root; the caller never supplies it.
    workspace: String,
    #[serde(default)]
    paths: Vec<String>,
}

/// `POST /cli/fs/resolve-links` (Thread file links, 0.46.1): which of the
/// path-shaped strings in a Thread message lead to a file or folder inside
/// the message's workspace. Read-only; POST because the batch is a body.
/// Symlinks are followed and anything whose real path leaves the workspace
/// (or is missing) answers a bare `linkable: false`
/// (`k2_core::file_links`). `prefixes` are the root's spellings the
/// renderer may use to tokenize a path with spaces in it.
pub fn handle_resolve_links(body: &[u8]) -> CliResponse {
    let parsed: ResolveLinksBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let token = parsed.workspace.trim();
    if token.is_empty() {
        return CliResponse::bad_request("missing 'workspace'");
    }
    // ticket-thread-v1: `ticket:<id>` names the ticket's workspace.
    let ticket_root = match crate::overlay_routes::ticket_addr::workspace_of(token, None) {
        Some(Ok((_, root))) => Some(root),
        Some(Err(r)) => return r,
        None => None,
    };
    let Some(root) = ticket_root.or_else(|| crate::workspace_msg::resolve_workspace_allowing_handle(token)) else {
        return CliResponse {
            status: "404 Not Found",
            content_type: "application/json",
            body: r#"{"error":"workspace not found"}"#.to_string(),
        };
    };
    let root = PathBuf::from(root);
    let home = k2_core::paths::home_dir();
    let links = k2_core::file_links::resolve_links(&root, home.as_deref(), &parsed.paths);
    CliResponse::ok_json(
        serde_json::json!({
            "root": root.to_string_lossy(),
            "prefixes": k2_core::file_links::root_prefixes(&root, home.as_deref()),
            "links": links,
        })
        .to_string(),
    )
}

/// `POST /cli/fs/resolve-links` for an App guest or App pass (App parity,
/// 0.46.3; catalog `files.resolveLinks`): the same answer shape, cut down
/// to what its own file rights cover. `workspace` must be one of its rooms
/// (else the uniform `skin_room`), it needs `files:read` there, and a
/// candidate is linkable only when its real path is inside the room, is
/// not a secret, and sits under its folder scope. The answer never carries
/// the room's host path (`root`, `prefixes`, `path`): only `rel`. A `~`
/// path is never expanded.
pub fn handle_resolve_links_guest(body: &[u8], pass: &SkinPass) -> CliResponse {
    let parsed: ResolveLinksBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    // ticket-thread-v1: `ticket:<id>` names the ticket's room (with
    // tickets:read there); files:read there still decides every link.
    let ws = match crate::overlay_routes::ticket_addr::workspace_of(&parsed.workspace, Some(pass)) {
        Some(Ok((project_id, _))) => match lookup_project(&project_id) {
            Some((path, handle)) => SkinWorkspace { project_id, path, handle },
            None => return crate::skin_routes::skin_room_response(),
        },
        Some(Err(r)) => return r,
        None => match resolve_skin_workspace(pass, &parsed.workspace) {
            Ok(w) => w,
            Err(r) => return r,
        },
    };
    if !pass.has_cap_in_room(&ws.project_id, crate::skin_routes::FILES_READ) {
        return crate::thread_attachments::cap_refused(crate::skin_routes::FILES_READ);
    }
    let root = PathBuf::from(&ws.path);
    let links: Vec<serde_json::Value> = k2_core::file_links::resolve_links(&root, None, &parsed.paths)
        .into_iter()
        .map(|a| {
            let rel = a.rel.clone().unwrap_or_default();
            let ok = a.linkable
                && !rel.is_empty()
                && !k2_core::secret_paths::is_secret_rel(&rel)
                && guest_may_read_rel(pass, &ws.project_id, &ws.path, &rel);
            if ok {
                serde_json::json!({ "input": a.input, "linkable": true, "kind": a.kind, "rel": rel })
            } else {
                serde_json::json!({ "input": a.input, "linkable": false })
            }
        })
        .collect();
    CliResponse::ok_json(serde_json::json!({ "links": links }).to_string())
}

#[derive(Deserialize)]
struct WriteFileBody {
    path: String,
    content: String,
    /// The file as the caller last read it: the SHA-256 (64 lowercase hex)
    /// of its bytes, or [`EXPECT_ABSENT`] for a file that wasn't there.
    #[serde(default)]
    expect: Option<String>,
}

/// `expect` for a file the caller saw missing (a new file): the write goes
/// only while nothing is there.
pub const EXPECT_ABSENT: &str = "absent";

/// SHA-256 of `bytes`, 64 lowercase hex.
pub fn content_sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// `fs/write-file`'s `expect` (the editors' autosave, so it never writes
/// over another program's save): the file at `path` must still be what the
/// caller read: its bytes' SHA-256, or, with [`EXPECT_ABSENT`], no file.
/// Otherwise 409 and nothing is written:
/// - `file_changed`: another save landed since; `current: {sha256,
///   content?}` (the text as `fs/read-file` gives it, when it can and
///   `with_content`: the owner and Connect logins, never an App guest) so the
///   editor can merge without another read;
/// - `file_missing`: the file is gone (it expected one);
/// - `file_exists`: it expected none and there is one (`current` as above).
///
/// A malformed `expect` is a 400. No `expect`: no check (as before). The
/// check runs right before the write: a save in the same instant can still
/// cross it, which the editor's next check sees.
pub fn expect_unchanged(path: &str, expect: Option<&str>, with_content: bool) -> Result<(), CliResponse> {
    let Some(expect) = expect.map(str::trim) else { return Ok(()) };
    let absent = expect == EXPECT_ABSENT;
    if !absent && !(expect.len() == 64 && expect.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))) {
        return Err(CliResponse::bad_request(format!(
            "expect is the SHA-256 (64 lowercase hex) of the file as you read it, or \"{EXPECT_ABSENT}\""
        )));
    }
    let conflict = |code: &str, error: &str, current: Option<serde_json::Value>| {
        let mut body = serde_json::json!({ "error": error, "code": code });
        if let Some(c) = current {
            body["current"] = c;
        }
        CliResponse { status: "409 Conflict", content_type: "application/json", body: body.to_string() }
    };
    let current = |bytes: &[u8]| {
        let mut c = serde_json::json!({ "sha256": content_sha256(bytes) });
        if let Some(f) = with_content.then(|| fsc::read_file_coded(path).ok()).flatten() {
            c["content"] = serde_json::json!(f.content);
        }
        c
    };
    let on_disk = match fsc::validate_path(path) {
        Ok(p) => std::fs::read(p).ok(),
        Err(e) => return Err(CliResponse::bad_request(e)),
    };
    match (on_disk, absent) {
        (None, true) => Ok(()),
        (Some(bytes), true) => Err(conflict("file_exists", "a file is there now; read it before you write", Some(current(&bytes)))),
        (None, false) => Err(conflict("file_missing", "the file is gone since you read it", None)),
        (Some(bytes), false) if content_sha256(&bytes) == expect => Ok(()),
        (Some(bytes), false) => Err(conflict("file_changed", "the file changed on disk since you read it", Some(current(&bytes)))),
    }
}

pub fn handle_write_file(body: &[u8]) -> CliResponse {
    let parsed: WriteFileBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if let Err(r) = expect_unchanged(&parsed.path, parsed.expect.as_deref(), true) {
        return r;
    }
    match fsc::write_file(&parsed.path, &parsed.content) {
        Ok(()) => {
            crate::session_events::emit_fs_changed_for_paths([parsed.path]);
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

// ── Skin jail (workspace-relative; never host-wide validate_path) ─────

/// Resolved workspace for a skin/owner files request.
pub struct SkinWorkspace {
    #[allow(dead_code)]
    pub project_id: String,
    pub path: String,
    pub handle: String,
}

/// Jail `rel` under `ws_root`. Relative only; reject empty, abs, `..`,
/// symlink escape. `for_write` allows a not-yet-created leaf whose parent
/// stays inside the root.
pub fn jail_rel_path(ws_root: &str, rel: &str, for_write: bool) -> Result<PathBuf, String> {
    let rel = rel.trim();
    if rel.is_empty() {
        return Err("empty path".to_string());
    }
    let p = Path::new(rel);
    if p.is_absolute() {
        return Err("path must be relative to the workspace".to_string());
    }
    if p.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        )
    }) {
        return Err("path must not contain '..'".to_string());
    }
    let root = dunce::canonicalize(&Path::new(ws_root))
        .map_err(|e| format!("workspace root unavailable: {e}"))?;
    // `root.join(".")` collapses: components equal root, so `.parent()` is
    // the host parent (escapes). Treat CurDir-only as the workspace root.
    if p.components().all(|c| matches!(c, Component::CurDir)) {
        return Ok(root);
    }
    let target = root.join(p);
    if for_write {
        let parent = target
            .parent()
            .ok_or_else(|| "path has no parent directory".to_string())?;
        if parent.exists() {
            let parent = dunce::canonicalize(&parent)
                .map_err(|e| format!("Invalid path: {e}"))?;
            if !parent.starts_with(&root) {
                return Err("path escapes the workspace".to_string());
            }
        } else if parent != root && !parent.starts_with(&root) {
            return Err("path escapes the workspace".to_string());
        }
        // F23: parent jail is enough for a new leaf. An existing leaf
        // symlink would let write_file follow it outside the jail.
        match std::fs::symlink_metadata(&target) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err("path escapes the workspace".to_string());
            }
            _ => {}
        }
        Ok(target)
    } else {
        let canon = dunce::canonicalize(&target).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                missing_path_text()
            } else {
                format!("Invalid path: {e}")
            }
        })?;
        if !canon.starts_with(&root) {
            return Err("path escapes the workspace".to_string());
        }
        Ok(canon)
    }
}

/// The jail's words for a path that isn't there (any OS's "not found").
/// A hidden path (a role line's `hide` list) answers exactly this on every
/// guest read door, so it reads like a missing file (no existence oracle).
pub(crate) fn missing_path_text() -> String {
    format!("Invalid path: {}", std::io::Error::from_raw_os_error(2))
}

/// The guest read doors' answer for a path that isn't there (and for a
/// hidden one): the jail's 400.
pub(crate) fn missing_path_response() -> CliResponse {
    CliResponse::bad_request(missing_path_text())
}

fn relative_under(root: &Path, abs: &str) -> String {
    Path::new(abs)
        .strip_prefix(root)
        .map(|p| {
            let s = p.to_string_lossy().replace('\\', "/");
            if s.is_empty() {
                ".".to_string()
            } else {
                s
            }
        })
        .unwrap_or_else(|_| abs.replace('\\', "/"))
}

fn lookup_project(id: &str) -> Option<(String, String)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    match k2_core::db::schema::Project::get(&conn, id) {
        Ok(p) => {
            let path = p.path.trim().to_string();
            if path.is_empty() {
                None
            } else {
                Some((path, p.handle.trim().to_string()))
            }
        }
        Err(_) => None,
    }
}

/// Skin: unresolvable or not in rooms → 403 `skin_room` (no existence oracle).
pub fn resolve_skin_workspace(pass: &SkinPass, token: &str) -> Result<SkinWorkspace, CliResponse> {
    let token = token.trim();
    if token.is_empty() {
        return Err(crate::skin_routes::skin_room_response());
    }
    let ids = match k2_core::skin::resolve_room_tokens(&[token.to_string()]) {
        Ok(ids) if ids.len() == 1 => ids,
        _ => return Err(crate::skin_routes::skin_room_response()),
    };
    let project_id = ids[0].clone();
    if !pass.has_room(&project_id) {
        return Err(crate::skin_routes::skin_room_response());
    }
    let Some((path, handle)) = lookup_project(&project_id) else {
        return Err(crate::skin_routes::skin_room_response());
    };
    Ok(SkinWorkspace {
        project_id,
        path,
        handle,
    })
}

/// Owner/Connect files WS: unknown workspace is 400 (existence is fine).
pub fn resolve_owner_workspace(token: &str) -> Result<SkinWorkspace, CliResponse> {
    let token = token.trim();
    if token.is_empty() {
        return Err(CliResponse::bad_request(
            "missing workspace query parameter",
        ));
    }
    let ids = match k2_core::skin::resolve_room_tokens(&[token.to_string()]) {
        Ok(ids) if ids.len() == 1 => ids,
        Ok(_) => return Err(CliResponse::bad_request("unknown workspace")),
        Err(_) => return Err(CliResponse::bad_request("unknown workspace")),
    };
    let project_id = ids[0].clone();
    let Some((path, handle)) = lookup_project(&project_id) else {
        return Err(CliResponse::bad_request("unknown workspace"));
    };
    Ok(SkinWorkspace {
        project_id,
        path,
        handle,
    })
}

pub(crate) fn need_workspace(params: &HashMap<String, String>) -> Result<String, CliResponse> {
    params
        .get("workspace")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CliResponse::bad_request("missing workspace query parameter"))
}

pub(crate) fn need_rel_path(params: &HashMap<String, String>) -> Result<String, CliResponse> {
    params
        .get("path")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CliResponse::bad_request("Missing 'path' parameter"))
}

pub fn handle_read_dir_gated(
    params: &HashMap<String, String>,
    skin: Option<SkinPass>,
) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_read_dir(params, &pass),
        None => handle_read_dir(params),
    }
}

pub fn handle_read_file_gated(
    params: &HashMap<String, String>,
    skin: Option<SkinPass>,
) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_read_file(params, &pass),
        None => handle_read_file(params),
    }
}

pub fn handle_write_file_gated(body: &[u8], skin: Option<SkinPass>) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_write_file(body, &pass),
        None => handle_write_file(body),
    }
}

pub fn handle_read_binary_gated(
    params: &HashMap<String, String>,
    skin: Option<SkinPass>,
) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_read_binary(params, &pass),
        None => handle_read_binary(params),
    }
}

pub fn handle_read_range_gated(
    params: &HashMap<String, String>,
    skin: Option<SkinPass>,
) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_read_range(params, &pass),
        None => handle_read_range(params),
    }
}

/// `GET /cli/fs/stat` (App verb `files.stat`): one path's kind, size,
/// times, extension and content type. Owner/Connect: an absolute `path`.
/// An App guest: `workspace` + a room-relative `path`, under the same room,
/// cap (`files:read`) and folder-scope rules as `read-dir`; `path` comes
/// back room-relative.
pub fn handle_stat_gated(params: &HashMap<String, String>, skin: Option<SkinPass>) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_stat(params, &pass),
        None => handle_stat(params),
    }
}

pub fn handle_stat(params: &HashMap<String, String>) -> CliResponse {
    let path = match params.get("path") {
        Some(p) if !p.is_empty() => p.clone(),
        _ => return CliResponse::bad_request("Missing 'path' parameter"),
    };
    match fsc::stat(&path) {
        Ok(st) => CliResponse::ok_json(serde_json::to_string(&st).unwrap_or_else(|_| "{}".to_string())),
        Err(e) => CliResponse::bad_request(e),
    }
}

fn handle_skin_stat(params: &HashMap<String, String>, pass: &SkinPass) -> CliResponse {
    let ws = match need_workspace(params) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let resolved = match resolve_skin_workspace(pass, &ws) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if !pass.has_cap_in_room(&resolved.project_id, crate::skin_routes::FILES_READ) {
        return crate::skin_routes::missing_cap_response(crate::skin_routes::FILES_READ);
    }
    let rel = match need_rel_path(params) {
        Ok(p) => p,
        Err(e) => return e,
    };
    // A folder on the way to a granted one stats like it lists (List).
    let jailed = match skin_jail_need(pass, &resolved, &rel, PathNeed::List) {
        Ok(p) => p,
        Err(r) => return r,
    };
    match fsc::stat(&jailed.to_string_lossy()) {
        Ok(mut st) => {
            let root = dunce::canonicalize(&Path::new(&resolved.path))
                .unwrap_or_else(|_| PathBuf::from(&resolved.path));
            st.path = relative_under(&root, &st.path);
            CliResponse::ok_json(serde_json::to_string(&st).unwrap_or_else(|_| "{}".to_string()))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_upload_binary_gated(body: &[u8], skin: Option<SkinPass>) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_upload_binary(body, &pass),
        None => handle_upload_binary(body),
    }
}

/// `POST /cli/fs/upload-chunk`: owner/Member keep the absolute-`dir` form
/// ([`handle_upload_chunk`]); an app guest gets the jailed, room-scoped,
/// principal-bound form ([`handle_skin_upload_chunk`]).
pub fn handle_upload_chunk_gated(body: &[u8], skin: Option<SkinPass>) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_upload_chunk(body, &pass),
        None => handle_upload_chunk(body),
    }
}

pub fn handle_create_gated(body: &[u8], skin: Option<SkinPass>) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_create(body, &pass),
        None => handle_create(body),
    }
}

pub fn handle_copy_gated(body: &[u8], skin: Option<SkinPass>) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_copy(body, &pass),
        None => handle_copy(body),
    }
}

pub fn handle_move_gated(body: &[u8], skin: Option<SkinPass>) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_move(body, &pass),
        None => handle_move(body),
    }
}

pub fn handle_rename_gated(body: &[u8], skin: Option<SkinPass>) -> CliResponse {
    match skin {
        Some(pass) => handle_skin_rename(body, &pass),
        None => handle_rename(body),
    }
}

fn handle_skin_read_dir(params: &HashMap<String, String>, pass: &SkinPass) -> CliResponse {
    let ws = match need_workspace(params) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let resolved = match resolve_skin_workspace(pass, &ws) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if !pass.has_cap_in_room(&resolved.project_id, crate::skin_routes::FILES_READ) {
        return crate::skin_routes::missing_cap_response(crate::skin_routes::FILES_READ);
    }
    let rel = match need_rel_path(params) {
        Ok(p) => p,
        Err(e) => return e,
    };
    // 0.46.16: a hidden folder (the line's `hide` list) is the 404 of a
    // folder that isn't there, before anything else is looked at.
    let scope = match room_scope(pass, &resolved, PathNeed::List) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if k2_core::skin::paths::hides_rel(&scope, &rel) {
        return read_dir_not_found();
    }
    if let Some(r) = scope_lexical(&scope, &rel, PathNeed::List) {
        return r;
    }
    let jailed = match jail_rel_path(&resolved.path, &rel, false) {
        Ok(p) => p,
        Err(e) => {
            // 0.46.6: a folder that isn't there is a 404 — after the
            // path-scope check on where it would land, so a guest never
            // learns what is (or isn't) in a folder it can't list.
            let Some(would_be) = missing_in_room(&resolved.path, &rel) else {
                return CliResponse::bad_request(e);
            };
            if hidden_landed(&scope, &resolved, &would_be) {
                return read_dir_not_found();
            }
            if let Some(r) = scope_gate_landed(&scope, &resolved, &would_be, PathNeed::List, true) {
                return r;
            }
            return read_dir_not_found();
        }
    };
    if hidden_landed(&scope, &resolved, &jailed) {
        return read_dir_not_found();
    }
    if let Some(r) = scope_gate_landed(&scope, &resolved, &jailed, PathNeed::List, true) {
        return r;
    }
    let show_hidden = matches!(
        params.get("show_hidden").map(|v| v.as_str()),
        Some("1") | Some("true") | Some("on")
    );
    let abs = jailed.to_string_lossy().to_string();
    match fsc::read_dir_coded(&abs, show_hidden) {
        Ok(mut entries) => {
            let root = dunce::canonicalize(&Path::new(&resolved.path))
                .unwrap_or_else(|_| PathBuf::from(&resolved.path));
            // 0.46.16: a link whose target is hidden is left out too (its
            // name alone isn't, so the landed path is asked).
            if !scope.hide.is_empty() {
                entries.retain(|e| !hidden_landed(&scope, &resolved, Path::new(&e.path)));
            }
            for e in &mut entries {
                e.path = relative_under(&root, &e.path);
            }
            // R3: a scoped line lists only the granted folders and the
            // way down to them (never a sibling); hidden paths are never
            // listed, `show_hidden` or not.
            if scope.has_walls() {
                entries.retain(|e| {
                    k2_core::skin::paths::allows_rel(&scope, &e.path, PathNeed::List).unwrap_or(false)
                });
            }
            CliResponse::ok_json(
                serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string()),
            )
        }
        Err(e) => read_dir_refusal(e),
    }
}

fn handle_skin_read_file(params: &HashMap<String, String>, pass: &SkinPass) -> CliResponse {
    let ws = match need_workspace(params) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let resolved = match resolve_skin_workspace(pass, &ws) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if !pass.has_cap_in_room(&resolved.project_id, crate::skin_routes::FILES_READ) {
        return crate::skin_routes::missing_cap_response(crate::skin_routes::FILES_READ);
    }
    let rel = match need_rel_path(params) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let jailed = match skin_jail_read(pass, &resolved, &rel) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let abs = jailed.to_string_lossy().to_string();
    match fsc::read_file_coded(&abs) {
        Ok(mut content) => {
            content.path = rel.replace('\\', "/");
            CliResponse::ok_json(
                serde_json::to_string(&content).unwrap_or_else(|_| "{}".to_string()),
            )
        }
        Err(e) => read_file_refusal(e),
    }
}

fn handle_skin_write_file(body: &[u8], pass: &SkinPass) -> CliResponse {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let workspace = v
        .get("workspace")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if workspace.is_empty() {
        return CliResponse::bad_request("missing workspace");
    }
    let rel = v
        .get("path")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if rel.is_empty() {
        return CliResponse::bad_request("Missing 'path' parameter");
    }
    let content = match v.get("content").and_then(|x| x.as_str()) {
        Some(s) => s.to_string(),
        None => return CliResponse::bad_request("Missing 'content' parameter"),
    };
    let resolved = match resolve_skin_workspace(pass, &workspace) {
        Ok(r) => r,
        Err(e) => return e,
    };
    // Rosson R2: `.k2/heartbeats/**` is not files:write ground. Only each
    // heartbeat's WAKEUP.md, and only with heartbeats:write.
    if crate::heartbeat_app_routes::lexically_in_heartbeats(&rel) {
        return skin_write_heartbeat_wakeup(pass, &resolved, &rel, &content);
    }
    if !pass.has_cap_in_room(&resolved.project_id, crate::skin_routes::FILES_WRITE) {
        return crate::skin_routes::missing_cap_response(crate::skin_routes::FILES_WRITE);
    }
    let scope = match scope_gate(pass, &resolved, &rel, PathNeed::Write) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let jailed = match jail_rel_path(&resolved.path, &rel, true) {
        Ok(p) => p,
        Err(e) => return CliResponse::bad_request(e),
    };
    if let Some(r) = refuse_protected_write(pass, &scope, &resolved, &rel, &jailed, false) {
        return r;
    }
    let abs = jailed.to_string_lossy().to_string();
    if let Err(r) = expect_unchanged(&abs, v.get("expect").and_then(|x| x.as_str()), false) {
        return r;
    }
    match fsc::write_file(&abs, &content) {
        Ok(()) => {
            crate::session_events::emit_fs_changed_for_paths([abs]);
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// R2 refusal: a files:write op that reaches `.k2/heartbeats/`.
pub fn heartbeats_area_response() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({
            "error": "heartbeats_area",
            "message": ".k2/heartbeats/ is managed by heartbeats; edit a heartbeat's WAKEUP.md with heartbeats:write",
        })
        .to_string(),
    }
}

/// R2 — `heartbeats:write` edits exactly `.k2/heartbeats/<name>/WAKEUP.md`
/// of an existing heartbeat in the room, and nothing else in `.k2/`.
fn skin_write_heartbeat_wakeup(
    pass: &SkinPass,
    resolved: &SkinWorkspace,
    rel: &str,
    content: &str,
) -> CliResponse {
    let Some(name) = crate::heartbeat_app_routes::exact_wakeup_name(rel) else {
        return heartbeats_area_response();
    };
    if !pass.has_cap_in_room(&resolved.project_id, k2_core::skin::CAP_HEARTBEATS_WRITE) {
        return crate::skin_routes::missing_cap_response(k2_core::skin::CAP_HEARTBEATS_WRITE);
    }
    let expected_rel = format!(".k2/heartbeats/{name}/WAKEUP.md");
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::AgentHeartbeat::get_by_name(&conn, &resolved.project_id, &name)
    };
    match row {
        Ok(Some(h)) if h.archived_at.is_none() => {
            // Windows: a row written before 0.46.7 (or not yet repaired at
            // boot) may hold `.k2\heartbeats\..`; compare the `/` form.
            #[cfg(windows)]
            let stored = h.wakeup_path.replace('\\', "/");
            #[cfg(not(windows))]
            let stored = h.wakeup_path.clone();
            if stored.trim_start_matches("./") != expected_rel {
                return heartbeats_area_response();
            }
        }
        Ok(_) => {
            return crate::heartbeat_app_routes::no_such_heartbeat_response(
                &k2_core::heartbeats::no_such_heartbeat(&name),
            )
        }
        Err(e) => return CliResponse::internal_error(e),
    }
    let jailed = match jail_rel_path(&resolved.path, &expected_rel, true) {
        Ok(p) => p,
        Err(e) => return CliResponse::bad_request(e),
    };
    // A symlinked heartbeat folder must not turn this into a write
    // elsewhere in the room: the parent must be that heartbeat's folder.
    let want_parent = dunce::canonicalize(&Path::new(&resolved.path))
        .map(|r| r.join(".k2").join("heartbeats").join(&name));
    let got_parent = jailed.parent().map(|p| dunce::canonicalize(&p));
    match (want_parent, got_parent) {
        (Ok(want), Some(Ok(got))) if want == got => {}
        _ => return heartbeats_area_response(),
    }
    let abs = jailed.to_string_lossy().to_string();
    match fsc::write_file(&abs, content) {
        Ok(()) => {
            crate::session_events::emit_fs_changed_for_paths([abs]);
            if let Err(e) = k2_core::heartbeats::wait::refresh_project(&resolved.path) {
                k2_core::log_debug!("[fs/write-file] heartbeat wait refresh: {e}");
            }
            k2_core::heartbeats::record_change(
                &resolved.path,
                &name,
                "instructions edited",
                Some(&crate::routes::http::app_actor_label(pass)),
            );
            crate::heartbeat_routes::broadcast_roster(&resolved.path);
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// R2: refuse when `rel` (lexically) or its jailed absolute path reaches
/// `.k2/heartbeats/`. `ancestors` also refuses `.` and `.k2` (a move or
/// rename source that would carry the folder away).
fn refuse_heartbeats_area(
    resolved: &SkinWorkspace,
    rel: &str,
    abs: &Path,
    ancestors: bool,
) -> Option<CliResponse> {
    let lexical = if ancestors {
        crate::heartbeat_app_routes::lexically_touches_heartbeats(rel)
    } else {
        crate::heartbeat_app_routes::lexically_in_heartbeats(rel)
    };
    if lexical || crate::heartbeat_app_routes::abs_touches_heartbeats(&resolved.path, abs, ancestors) {
        Some(heartbeats_area_response())
    } else {
        None
    }
}

// ── Folder path scope (room-scope R3, RS-A1..A3) ───────────────────────
//
// A role's room line may list the folders its logins reach in that room
// (`k2 app role paths`). Every guest file door checks the room-relative
// path BEFORE touching the disk ([`scope_gate`]), so an unlisted path is
// the same 403 as a missing cap whether or not it exists (RS-A3), and
// checks the landed path again after the jail with symlinks followed
// ([`scope_gate_landed`]), so a link inside a granted folder can't reach
// a sibling. A scope that can't be read fails closed with that same 403.
// The owner token, Connect users and agents' own disk reads never carry a
// `SkinPass` and never take this path (RS-A4).

pub(crate) use k2_core::skin::paths::PathNeed;
use k2_core::skin::RoomPathScope;

/// The one refusal: exactly the missing-cap 403 (no existence oracle).
fn scope_refused(need: PathNeed) -> CliResponse {
    crate::skin_routes::missing_cap_response(match need {
        PathNeed::Write => crate::skin_routes::FILES_WRITE,
        PathNeed::List | PathNeed::Read => crate::skin_routes::FILES_READ,
        PathNeed::Upload => k2_core::skin::CAP_FILES_UPLOAD,
    })
}

/// A hidden path's refusal (0.46.16): a read or a listing answers exactly
/// like a path that isn't there (the jail's 400; `read-dir` answers its own
/// 404 itself), so a guest never learns whether it exists. A write, upload,
/// copy, move or rename into one is the plain missing-cap 403 (decided on
/// the path alone, never on the disk).
fn hidden_refused(need: PathNeed) -> CliResponse {
    match need {
        PathNeed::List | PathNeed::Read => missing_path_response(),
        PathNeed::Write | PathNeed::Upload => scope_refused(need),
    }
}

/// `abs` (jailed) lands, symlinks followed, on a path the line hides. A
/// path that can't be resolved is not "hidden" here (the other gates fail
/// closed on it).
fn hidden_landed(scope: &RoomPathScope, resolved: &SkinWorkspace, abs: &Path) -> bool {
    if scope.hide.is_empty() {
        return false;
    }
    resolve_existing_prefix(abs)
        .as_deref()
        .and_then(|f| rel_in_room(&resolved.path, f))
        .is_some_and(|rel| k2_core::skin::paths::hides_rel(scope, &rel))
}

/// A copy, move or rename source that holds a rooted hidden path below it
/// (0.46.16): carrying it elsewhere would bring the hidden path out from
/// under its pattern. Checked on the path as sent and where it lands.
fn source_holds_hidden(scope: &RoomPathScope, resolved: &SkinWorkspace, rel: &str, abs: &Path) -> bool {
    if scope.hide.is_empty() {
        return false;
    }
    let holds = |r: &str| {
        k2_core::skin::paths::rel_segments(r).is_ok_and(|segs| k2_core::skin::paths::holds_hidden(scope, &segs))
    };
    if holds(rel) {
        return true;
    }
    match resolve_existing_prefix(abs).as_deref().and_then(|f| rel_in_room(&resolved.path, f)) {
        Some(landed) => holds(&landed),
        None => true,
    }
}

/// This login's folders on the resolved room line. `Err` = fail closed.
pub(crate) fn room_scope(pass: &SkinPass, resolved: &SkinWorkspace, need: PathNeed) -> Result<RoomPathScope, CliResponse> {
    pass.path_scope(&resolved.project_id).map_err(|e| {
        k2_core::log_debug!("[fs] path scope unreadable for pass {}: {e}", pass.id);
        scope_refused(need)
    })
}

/// Lexical check of one room-relative path, before any disk access. A
/// malformed path answers the jail's own 400 (the same an unscoped login
/// gets).
fn scope_lexical(scope: &RoomPathScope, rel: &str, need: PathNeed) -> Option<CliResponse> {
    if k2_core::skin::paths::hides_rel(scope, rel) {
        return Some(hidden_refused(need));
    }
    match k2_core::skin::paths::allows_rel(scope, rel, need) {
        Ok(true) => None,
        Ok(false) => Some(scope_refused(need)),
        Err(e) => Some(CliResponse::bad_request(e)),
    }
}

/// [`room_scope`] + [`scope_lexical`]: the gate every guest file door runs
/// first. Returns the scope for the landed-path check.
fn scope_gate(
    pass: &SkinPass,
    resolved: &SkinWorkspace,
    rel: &str,
    need: PathNeed,
) -> Result<RoomPathScope, CliResponse> {
    let scope = room_scope(pass, resolved, need)?;
    match scope_lexical(&scope, rel, need) {
        Some(r) => Err(r),
        None => Ok(scope),
    }
}

/// The room-relative form of an already resolved absolute path, or `None`
/// when it isn't under the room.
pub(crate) fn rel_in_room(room_root: &str, full: &Path) -> Option<String> {
    let root = dunce::canonicalize(&Path::new(room_root)).ok()?;
    let rest = full.strip_prefix(&root).ok()?;
    let s = rest.to_string_lossy().replace('\\', "/");
    Some(if s.is_empty() { ".".to_string() } else { s })
}

/// After the jail: the landed path (symlinks followed; for a new name the
/// leaf is kept as written) must still be inside the scope and not hidden.
/// Lines with no folders and nothing hidden pass. Anything that can't be
/// resolved fails closed.
pub(crate) fn scope_gate_landed(
    scope: &RoomPathScope,
    resolved: &SkinWorkspace,
    abs: &Path,
    need: PathNeed,
    follow_leaf: bool,
) -> Option<CliResponse> {
    if !scope.has_walls() {
        return None;
    }
    let full = if follow_leaf { resolve_existing_prefix(abs) } else { resolve_parent_keep_leaf(abs) };
    let Some(rel) = full.as_deref().and_then(|f| rel_in_room(&resolved.path, f)) else {
        return Some(scope_refused(need));
    };
    if k2_core::skin::paths::hides_rel(scope, &rel) {
        return Some(hidden_refused(need));
    }
    match k2_core::skin::paths::allows_rel(scope, &rel, need) {
        Ok(true) => None,
        _ => Some(scope_refused(need)),
    }
}

/// Both checks for one write target (`rel` as sent, `abs` jailed). `need`
/// is `Write`, or `Upload` for an upload-only drop (AC10).
fn scope_write_target(
    scope: &RoomPathScope,
    resolved: &SkinWorkspace,
    rel: &str,
    abs: &Path,
    follow_leaf: bool,
    need: PathNeed,
) -> Option<CliResponse> {
    scope_lexical(scope, rel, need).or_else(|| scope_gate_landed(scope, resolved, abs, need, follow_leaf))
}

// ── Agent config folder (GH#72) ────────────────────────────────────────
//
// Rosson 2026-10-14: an App guest's `files:write` stops at the room's
// `.k2/` folder (the agent's own config: its role, inbox, skills) unless
// the guest's grant carries the opt-in `files:agentconfig` for that room.
// Reads are unchanged. Every guest write door (write-file, upload-binary,
// upload-chunk, create, copy, move, rename) goes through
// [`refuse_protected_write`], so this is the one check. Guests have no
// delete door. The owner token, Connect users, agent sessions and the
// daemon never take this path (they never carry a `SkinPass`).

/// `true` when one path segment names the agent config folder under any
/// of its names (`.k2`, the older `.k2so`, the coming `.k5`), ASCII
/// case-folded (`.K2` on a case-insensitive disk is the same folder), with
/// trailing dots/spaces ignored (Windows strips them). A workspace can
/// keep a `.k2so/` beside its `.k2/`, with root links (`CLAUDE.md`,
/// `.claude/skills/…`) still pointing into it: a write there changes what
/// the agent loads just as a write under `.k2/` does.
fn is_agent_config_segment(seg: &str) -> bool {
    k2_core::paths::is_workspace_dot_dir(seg)
}

/// `true` when a room-root name is one a guest's `files:write` alone doesn't
/// reach: the config folder, or a file the agent's CLI loads as standing orders.
/// Writes only: what a guest may read or download is unchanged.
fn is_write_protected_root_name(seg: &str) -> bool {
    is_agent_config_segment(seg) || k2_core::skin::paths::is_root_instruction_file(seg)
}

/// Normal segments of a room-relative path, with `/` and `\` both
/// treated as separators and `.` dropped.
fn rel_segments(rel: &str) -> Vec<&str> {
    rel.trim()
        .split(['/', '\\'])
        .filter(|s| !s.is_empty() && *s != ".")
        .collect()
}

/// Lexical: `rel` is `.k2` or under it. With `ancestors`, the room root
/// itself counts too (moving or renaming it would carry `.k2/` along).
pub fn lexically_in_agent_config(rel: &str, ancestors: bool) -> bool {
    let segs = rel_segments(rel);
    match segs.first() {
        Some(first) => is_agent_config_segment(first),
        None => ancestors,
    }
}

/// Lexical: `rel` is at or under a name [`is_write_protected_root_name`] holds.
fn lexically_write_protected(rel: &str, ancestors: bool) -> bool {
    match rel_segments(rel).first() {
        Some(first) => is_write_protected_root_name(first),
        None => ancestors,
    }
}

/// `abs` with every existing prefix resolved (symlinks followed) and the
/// not-yet-created tail kept as written. `None` when an existing prefix
/// can't be resolved (a dangling symlink): callers fail closed.
fn resolve_existing_prefix(abs: &Path) -> Option<PathBuf> {
    resolve_existing_prefix_seam(abs, |_| {})
}

/// [`resolve_existing_prefix`], with `between` run after a failed
/// canonicalize and before the existence check: the window a parallel
/// request can create the folder in. `between` must be a no-op in
/// production (only [`resolve_existing_prefix`] calls this outside tests);
/// tests create a folder or a link there.
fn resolve_existing_prefix_seam(abs: &Path, mut between: impl FnMut(&Path)) -> Option<PathBuf> {
    let with_tail = |mut out: PathBuf, tail: &[std::ffi::OsString]| {
        for seg in tail.iter().rev() {
            out.push(seg);
        }
        out
    };
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = abs.to_path_buf();
    loop {
        match dunce::canonicalize(&cur) {
            Ok(out) => return Some(with_tail(out, &tail)),
            Err(_) => {
                between(&cur);
                if std::fs::symlink_metadata(&cur).is_ok() {
                    // Something is there now: it appeared after the
                    // canonicalize (a parallel upload making the guest's
                    // drop folder), or it is a dangling or looping link.
                    // Resolve it exactly once more and return only what
                    // that says; the caller runs the whole protected-path
                    // decision on it (a link into `.k2` is refused there).
                    // Still unresolvable: fail closed.
                    return dunce::canonicalize(&cur).ok().map(|out| with_tail(out, &tail));
                }
                let name = cur.file_name()?.to_owned();
                let parent = cur.parent()?.to_path_buf();
                tail.push(name);
                cur = parent;
            }
        }
    }
}

/// Resolved: `abs` (already jailed) lands at or under the room's `.k2/`
/// once symlinks are followed, compared ASCII case-folded. With
/// `ancestors`, the room root itself counts too.
pub fn abs_in_agent_config(root: &str, abs: &Path, ancestors: bool) -> bool {
    resolved_in_agent_config(root, resolve_existing_prefix(abs).as_deref(), ancestors)
}

/// `abs` with its parent resolved (symlinks followed) and the LAST segment
/// kept as written, never followed. `None` as in [`resolve_existing_prefix`].
fn resolve_parent_keep_leaf(abs: &Path) -> Option<PathBuf> {
    let name = abs.file_name()?;
    resolve_existing_prefix(abs.parent()?).map(|p| p.join(name))
}

/// [`abs_in_agent_config`] on an already resolved target (`None` = could
/// not be resolved: fails closed).
fn resolved_in_agent_config(root: &str, target: Option<&Path>, ancestors: bool) -> bool {
    resolved_first_segment(root, target, ancestors, is_agent_config_segment)
}

/// [`resolved_in_agent_config`] for a write: [`is_write_protected_root_name`].
fn resolved_write_protected(root: &str, target: Option<&Path>, ancestors: bool) -> bool {
    resolved_first_segment(root, target, ancestors, is_write_protected_root_name)
}

fn resolved_first_segment(root: &str, target: Option<&Path>, ancestors: bool, protected: fn(&str) -> bool) -> bool {
    let Ok(canon_root) = dunce::canonicalize(&root) else {
        return true;
    };
    let Some(target) = target else {
        return true;
    };
    let fold = |p: &Path| p.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
    let root_s = fold(&canon_root);
    let target_s = fold(target);
    if target_s == root_s {
        return ancestors;
    }
    match target_s.strip_prefix(&format!("{}/", root_s.trim_end_matches('/'))) {
        Some(rest) => rest
            .split('/')
            .find(|s| !s.is_empty() && *s != ".")
            .is_some_and(protected),
        // Jailed paths stay under the room; anything else fails closed.
        None => true,
    }
}

/// 403 for a guest write into the agent's config folder.
pub fn agent_config_protected_response() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({
            "error": "agent_config_protected",
            "message": "the agent's .k2 folder and its root CLAUDE.md, AGENTS.md and GEMINI.md are protected: app guests can't write them unless their role holds files:agentconfig for this room",
        })
        .to_string(),
    }
}

/// The one write check for every guest file door. `rel` is the
/// room-relative path the guest sent (or the landed path, e.g.
/// `<dir>/<filename>`), `abs` its jailed absolute path. `ancestors` is set
/// for a move/rename source (the root carries `.k2/` along). R2 heartbeat
/// ground is checked first and holds even with the opt-in.
fn refuse_protected_write(
    pass: &SkinPass,
    scope: &RoomPathScope,
    resolved: &SkinWorkspace,
    rel: &str,
    abs: &Path,
    ancestors: bool,
) -> Option<CliResponse> {
    refuse_protected_write_at(pass, scope, resolved, rel, abs, ancestors, true, PathNeed::Write)
}

/// [`refuse_protected_write`] for an upload door, which acts with `need`
/// (`Write`, or `Upload` for an upload-only drop, AC10). `follow_leaf =
/// false` is for a NEW name that the door never writes through
/// (`refuse_protected_new_name` before 0.46.4): upload-chunk publishes
/// with a no-replace hard link (or a rename after a no-follow existence
/// check) and picks `name (1).ext` when anything, a symlink included,
/// already sits at `<dir>/<name>`. So the leaf is checked as written and
/// NOT followed. Following it made an existing leaf symlink that points
/// out of the room (resolved target not under the room → fail closed)
/// refuse an ordinary upload with `agent_config_protected`. The folder is
/// still resolved (the caller has also checked it on its own), and `.k2`
/// as the name is still refused.
fn refuse_protected_upload(
    pass: &SkinPass,
    scope: &RoomPathScope,
    resolved: &SkinWorkspace,
    rel: &str,
    abs: &Path,
    follow_leaf: bool,
    need: PathNeed,
) -> Option<CliResponse> {
    refuse_protected_write_at(pass, scope, resolved, rel, abs, false, follow_leaf, need)
}

#[allow(clippy::too_many_arguments)]
fn refuse_protected_write_at(
    pass: &SkinPass,
    scope: &RoomPathScope,
    resolved: &SkinWorkspace,
    rel: &str,
    abs: &Path,
    ancestors: bool,
    follow_leaf: bool,
    need: PathNeed,
) -> Option<CliResponse> {
    // R3 first: outside the line's write folders (an upload-only drop: its
    // drop folders) is the missing-cap 403, whatever else the path is.
    if let Some(r) = scope_write_target(scope, resolved, rel, abs, follow_leaf, need) {
        return Some(r);
    }
    if let Some(r) = refuse_heartbeats_area(resolved, rel, abs, ancestors) {
        return Some(r);
    }
    let full = if follow_leaf {
        resolve_existing_prefix(abs)
    } else {
        resolve_parent_keep_leaf(abs)
    };
    // R2 again on the fully resolved target: a symlinked folder followed
    // by not-yet-created segments is invisible to a plain canonicalize.
    if let Some(full) = full.as_deref() {
        if crate::heartbeat_app_routes::abs_touches_heartbeats(&resolved.path, full, ancestors) {
            return Some(heartbeats_area_response());
        }
    }
    // The `.k2/` opt-in widens `files:write` only; an upload-only drop
    // never reaches `.k2/`, whatever the line holds.
    if need != PathNeed::Upload && pass.has_cap_in_room(&resolved.project_id, k2_core::skin::CAP_FILES_AGENT_CONFIG) {
        return None;
    }
    if lexically_write_protected(rel, ancestors)
        || resolved_write_protected(&resolved.path, full.as_deref(), ancestors)
    {
        return Some(agent_config_protected_response());
    }
    None
}

/// Read doors (read-file, read-binary, read-range): the R3 gate on the
/// path as sent, the jail, then the gate on the landed path.
fn skin_jail_read(pass: &SkinPass, resolved: &SkinWorkspace, rel: &str) -> Result<PathBuf, CliResponse> {
    skin_jail_need(pass, resolved, rel, PathNeed::Read)
}

pub(crate) fn skin_jail_need(
    pass: &SkinPass,
    resolved: &SkinWorkspace,
    rel: &str,
    need: PathNeed,
) -> Result<PathBuf, CliResponse> {
    let scope = scope_gate(pass, resolved, rel, need)?;
    let jailed = jail_rel_path(&resolved.path, rel, false).map_err(CliResponse::bad_request)?;
    if let Some(r) = scope_gate_landed(&scope, resolved, &jailed, need, true) {
        return Err(r);
    }
    // A hard link has no target the jail could resolve: one inside the
    // guest's folders may be a private file (a sibling's, or outside the
    // room) under a harmless name. Refused like a path outside the folders,
    // as the download door does (round 2 A3).
    if need == PathNeed::Read && crate::fs_download::hard_linked_file(&jailed) {
        return Err(scope_refused(need));
    }
    Ok(jailed)
}

/// Thread attachments (0.46.1): may this guest read the room file `rel`
/// in room `project_id` (rooted at `root`)? The same rules as the guest
/// read doors: the room, `files:read` there, the R3 folder scope on the
/// path as written, and (when the path resolves now) on the landed path
/// too. A path that no longer resolves passes on the lexical check alone,
/// so a gone file inside the guest's folders shows as gone, not hidden;
/// it can never be opened (the open re-jails). "A pointer, not a grant":
/// nothing here widens what the guest's file doors already allow.
pub(crate) fn guest_may_read_rel(pass: &SkinPass, project_id: &str, root: &str, rel: &str) -> bool {
    if !pass.has_room(project_id) || !pass.has_cap_in_room(project_id, crate::skin_routes::FILES_READ) {
        return false;
    }
    let ws = SkinWorkspace { project_id: project_id.to_string(), path: root.to_string(), handle: String::new() };
    let Ok(scope) = scope_gate(pass, &ws, rel, PathNeed::Read) else {
        return false;
    };
    match jail_rel_path(root, rel, false) {
        // A hard link is never readable by a guest (see skin_jail_need).
        Ok(jailed) => {
            scope_gate_landed(&scope, &ws, &jailed, PathNeed::Read, true).is_none()
                && !crate::fs_download::hard_linked_file(&jailed)
        }
        Err(_) => true,
    }
}

fn skin_resolve_write(
    pass: &SkinPass,
    workspace: &str,
) -> Result<SkinWorkspace, CliResponse> {
    let resolved = resolve_skin_workspace(pass, workspace)?;
    if !pass.has_cap_in_room(&resolved.project_id, crate::skin_routes::FILES_WRITE) {
        return Err(crate::skin_routes::missing_cap_response(
            crate::skin_routes::FILES_WRITE,
        ));
    }
    Ok(resolved)
}

// ── Upload-only drops (prd-app-caps-0464-v1 AC7–AC10, 0.46.4) ─────────
//
// The two upload doors (upload-binary, upload-chunk) take `files:write`
// OR `files:upload` in the room. `files:write` keeps today's rules byte
// for byte (the folder sent or `uploadDir`, `PathNeed::Write`; the answer
// gains `size`). With `files:upload` and no `files:write`:
// - the folder sent (else `uploadDir`) must be a drop or write folder of
//   the room line (`PathNeed::Upload`); an unscoped line drops only into
//   its `uploadDir` (and below), and a line with nowhere to drop is the
//   missing-cap 403 with a hint, so "anywhere in the room" never happens;
// - the file lands in `<folder>/<username>/` (Q1: a per-uploader
//   subfolder), so the collision-free name only ever collides with the
//   guest's own files and the answer is no oracle about anyone else's;
// - only a login with a principal may drop (its username names the
//   subfolder); a platform pass is refused;
// - lexical and landed checks run on the subfolder and the file, `.k2/`
//   is never reached (the `files:agentconfig` opt-in doesn't apply), and
//   heartbeat ground stays refused.
// Every other files door still needs `files:read` / `files:write`, so an
// upload-only holder can't list, read, write, move or rename anything,
// its own uploads included (AC11).

/// How an upload door acts for this caller in the room.
enum UploadMode {
    /// `files:write`: today's rules.
    Write,
    /// `files:upload` without `files:write`: drop into the uploader's own
    /// subfolder (`user` = the login's normalized username).
    Drop { user: String },
}

/// The room and the upload mode, or the missing-cap 403 (`files:write`,
/// as today, for a caller with neither cap).
fn skin_resolve_upload(pass: &SkinPass, workspace: &str) -> Result<(SkinWorkspace, UploadMode), CliResponse> {
    let resolved = resolve_skin_workspace(pass, workspace)?;
    if pass.has_cap_in_room(&resolved.project_id, crate::skin_routes::FILES_WRITE) {
        return Ok((resolved, UploadMode::Write));
    }
    if !pass.has_cap_in_room(&resolved.project_id, k2_core::skin::CAP_FILES_UPLOAD) {
        return Err(crate::skin_routes::missing_cap_response(crate::skin_routes::FILES_WRITE));
    }
    let principal = pass.principal_id.as_deref().map(str::trim).filter(|p| !p.is_empty());
    let user = match (pass.session, principal) {
        (true, Some(_)) => k2_core::skin::normalize_username(&pass.username).ok(),
        _ => None,
    };
    match user {
        Some(user) => Ok((resolved, UploadMode::Drop { user })),
        None => Err(crate::skin_routes::missing_cap_response(k2_core::skin::CAP_FILES_UPLOAD)),
    }
}

/// 403 for an upload-only login whose room line has nowhere to drop: the
/// missing-cap answer plus how the owner adds a drop folder (says nothing
/// about any file).
fn no_drop_folder() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({
            "error": format!("missing capability {}", k2_core::skin::CAP_FILES_UPLOAD),
            "hint": "this room has no drop folder: ask the owner (k2 app role paths <role> <room> --upload <folder>)",
        })
        .to_string(),
    }
}

/// The scope an upload-only drop is checked against: a scoped line as is
/// (its drop and write folders take uploads), an unscoped line as its
/// `uploadDir` alone. `None` = nowhere to drop.
fn drop_scope(line: &RoomPathScope) -> Option<RoomPathScope> {
    if line.is_scoped() {
        return (!line.write.is_empty() || !line.upload.is_empty()).then(|| line.clone());
    }
    let dir = line.upload_dir.clone()?;
    Some(RoomPathScope { upload: vec![dir.clone()], upload_dir: Some(dir), hide: line.hide.clone(), ..Default::default() })
}

/// Where an upload door lands.
struct UploadTarget {
    /// The caller's own scope on the room line (the lexical and landed
    /// checks run against it).
    scope: RoomPathScope,
    /// The room-relative landing folder (an upload-only drop: inside the
    /// uploader's own folder under its drop root).
    dir: String,
    need: PathNeed,
    /// Set for an upload-only drop.
    drop: Option<DropTarget>,
}

/// An upload-only drop's own folder (review A1, A4).
struct DropTarget {
    /// Every drop and write folder of the room, across ALL roles: the
    /// roots a guest's own folder sits under (and the quota counts).
    roots: RoomPathScope,
    /// `<root>/<user>`, room-relative.
    user_dir: String,
    user: String,
}

/// Every room line of every role on room `project_id` (round-2 A1). A
/// store that can't be read fails closed (the missing-cap 403).
fn room_lines(project_id: &str) -> Result<Vec<RoomPathScope>, CliResponse> {
    let roles = k2_core::skin::list_roles().map_err(|e| {
        k2_core::log_debug!("[fs] room lines unreadable: {e}");
        scope_refused(PathNeed::Upload)
    })?;
    Ok(roles.into_iter().filter_map(|r| r.room_paths.get(project_id).cloned()).collect())
}

/// The roots scope of a room (round-2 A1): the drop and write folders of
/// every role line there (an unscoped line's `uploadDir` counts as a drop
/// folder), plus the caller's own drop scope.
fn room_roots(lines: &[RoomPathScope], own: &RoomPathScope) -> RoomPathScope {
    let mut roots = RoomPathScope::default();
    let push = |list: &mut Vec<String>, f: &String| {
        if !list.contains(f) {
            list.push(f.clone());
        }
    };
    for l in lines.iter().chain(std::iter::once(own)) {
        for f in &l.write {
            push(&mut roots.write, f);
        }
        for f in &l.upload {
            push(&mut roots.upload, f);
        }
        if !l.is_scoped() {
            if let Some(d) = &l.upload_dir {
                push(&mut roots.upload, d);
            }
        }
    }
    roots
}

fn upload_target(pass: &SkinPass, resolved: &SkinWorkspace, mode: &UploadMode, sent: String) -> Result<UploadTarget, CliResponse> {
    match mode {
        UploadMode::Write => {
            let scope = room_scope(pass, resolved, PathNeed::Write)?;
            let dir = upload_dir_or_default(sent, &scope)?;
            Ok(UploadTarget { scope, dir, need: PathNeed::Write, drop: None })
        }
        UploadMode::Drop { user } => {
            let line = room_scope(pass, resolved, PathNeed::Upload)?;
            let Some(scope) = drop_scope(&line) else { return Err(no_drop_folder()) };
            let folder = if sent.is_empty() {
                match scope.upload_dir.clone() {
                    Some(d) => d,
                    None => return Err(no_drop_folder()),
                }
            } else {
                sent
            };
            // The drop folder itself first (lexical, before the disk).
            if let Some(r) = scope_lexical(&scope, &folder, PathNeed::Upload) {
                return Err(r);
            }
            let segs = k2_core::skin::paths::rel_segments(&folder).map_err(CliResponse::bad_request)?;
            // Review A1 (+ round 2): always `<drop root>/<user>/<rest>`, the
            // root being the OUTERMOST drop or write folder holding `dir`
            // across every role of the room, so a guest's own folder is
            // always the first segment under a root, whichever role made
            // the folders: `dir` adds subfolders inside the guest's own
            // folder and can never reach another person's.
            let lines = room_lines(&resolved.project_id)?;
            let roots = room_roots(&lines, &scope);
            let Some(root) = k2_core::skin::paths::drop_root(&roots, &segs) else {
                return Err(scope_refused(PathNeed::Upload));
            };
            let root_len = root.split('/').count();
            let user_dir = format!("{root}/{user}");
            let mut landing = vec![root, user.clone()];
            landing.extend(segs[root_len..].iter().cloned());
            let landing = landing.join("/");
            // The landing must still be one of the caller's own drop places
            // (a drop folder nested inside another role's root can't be).
            if let Some(r) = scope_lexical(&scope, &landing, PathNeed::Upload) {
                return Err(r);
            }
            // A username that is (or holds) a folder some role lists is
            // refused: it isn't that guest's folder (round 2 A1).
            let user_segs: Vec<String> = user_dir.split('/').map(str::to_string).collect();
            let listed = lines.iter().flat_map(|l| {
                l.read.iter().chain(l.write.iter()).chain(l.upload.iter()).chain(l.download.iter()).chain(l.upload_dir.iter())
            });
            for f in listed {
                if k2_core::skin::paths::folder_holds(&user_segs, f) {
                    return Err(scope_refused(PathNeed::Upload));
                }
            }
            Ok(UploadTarget { scope, dir: landing, need: PathNeed::Upload, drop: Some(DropTarget { roots, user_dir, user: user.clone() }) })
        }
    }
}

/// The marker a guest's own drop folder carries (round 2 A1): its
/// username. A folder at `<root>/<user>` without it (made by the owner or
/// an agent) is not that guest's, and the drop is refused.
pub const DROP_USER_MARKER: &str = ".k2-drop-user";

/// Make (or check) the guest's own folder `<root>/<user>` and its marker.
/// Any problem is the plain missing-cap 403 (no oracle).
fn ensure_user_dir(resolved: &SkinWorkspace, scope: &RoomPathScope, d: &DropTarget) -> Result<(), CliResponse> {
    let refuse = |why: &str| {
        k2_core::log_debug!("[fs] drop refused for {}: {why}", d.user);
        scope_refused(PathNeed::Upload)
    };
    let Ok(root) = dunce::canonicalize(&Path::new(&resolved.path)) else { return Err(refuse("room root")) };
    let lexical = root.join(&d.user_dir);
    // Where it lands, before anything is made (a drop folder that links to
    // a sibling folder never gets a guest folder or marker there).
    if scope_gate_landed(scope, resolved, &lexical, PathNeed::Upload, true).is_some() {
        return Err(refuse("lands outside the drop folders"));
    }
    // One guest folder is made (and its marker written) at a time, so a
    // racing drop of the same guest never sees a folder without its marker.
    static MAKING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _making = MAKING.lock().unwrap_or_else(|p| p.into_inner());
    let existed = std::fs::symlink_metadata(&lexical).is_ok();
    let canon = jail_mkdir_p(&root, &d.user_dir).map_err(|e| refuse(&e))?;
    let marker = canon.join(DROP_USER_MARKER);
    if !existed {
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.custom_flags(libc::O_NOFOLLOW);
        }
        match o.open(&marker) {
            Ok(mut f) => {
                use std::io::Write as _;
                f.write_all(d.user.as_bytes()).map_err(|e| refuse(&format!("marker: {e}")))?;
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(refuse(&format!("marker: {e}"))),
        }
    }
    match std::fs::symlink_metadata(&marker) {
        Ok(m) if m.file_type().is_file() => {}
        _ => return Err(refuse("a folder without the guest's marker")),
    }
    match std::fs::read_to_string(&marker) {
        Ok(name) if name == d.user => Ok(()),
        _ => Err(refuse("another name's folder")),
    }
}

/// Review A4: per guest and room, the drop folders hold at most this many
/// files (`K2_DROP_MAX_FILES`) …
pub const DROP_MAX_FILES_DEFAULT: u64 = 1_000;
/// … and this many bytes in all, chunk parts included (`K2_DROP_MAX_BYTES`).
pub const DROP_MAX_BYTES_DEFAULT: u64 = 2 * 1024 * 1024 * 1024;

fn env_u64(key: &str, default: u64) -> u64 {
    match std::env::var(key).ok().map(|v| v.trim().parse::<u64>()) {
        Some(Ok(n)) if n > 0 => n,
        _ => default,
    }
}

/// The drop caps in force: `(files, bytes)`.
pub fn drop_limits() -> (u64, u64) {
    (env_u64("K2_DROP_MAX_FILES", DROP_MAX_FILES_DEFAULT), env_u64("K2_DROP_MAX_BYTES", DROP_MAX_BYTES_DEFAULT))
}

/// Files and bytes under `dir` (never following a symlink; a link counts
/// as one entry of its own size). Stops early once past `stop_files`.
fn tally(dir: &Path, files: &mut u64, bytes: &mut u64, stop_files: u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if *files > stop_files {
            return;
        }
        let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
        if m.file_type().is_dir() {
            tally(&e.path(), files, bytes, stop_files);
        } else if e.file_name() == DROP_USER_MARKER {
            // The guest-folder marker is K2's, not a drop.
        } else {
            *files += 1;
            *bytes += m.len();
        }
    }
}

/// The guests whose drop is being written, per room (round 2 A4): one
/// drop per guest and room at a time, so the count on disk is exact while
/// it is checked and written (parallel uploads can't overshoot the caps).
fn drop_busy() -> &'static (std::sync::Mutex<std::collections::HashSet<String>>, std::sync::Condvar) {
    static M: std::sync::OnceLock<(std::sync::Mutex<std::collections::HashSet<String>>, std::sync::Condvar)> =
        std::sync::OnceLock::new();
    M.get_or_init(|| (std::sync::Mutex::new(std::collections::HashSet::new()), std::sync::Condvar::new()))
}

/// A guest's turn to drop in a room; the next one waits until this request
/// ends (its file is on disk by then, or the write failed).
struct DropReservation {
    key: String,
}

impl Drop for DropReservation {
    fn drop(&mut self) {
        let (m, c) = drop_busy();
        let mut busy = m.lock().unwrap_or_else(|p| p.into_inner());
        busy.remove(&self.key);
        c.notify_all();
    }
}

/// Review A4 (+ round 2): take this guest's turn in this room, then check
/// the drop against its caps: every `<root>/<user>` of the room's roots on
/// disk, counted while no other drop of this guest is being written. The
/// turn is held until the request ends. The refusal names only the guest's
/// own totals.
fn drop_reserve(resolved: &SkinWorkspace, d: &DropTarget, add_files: u64, add_bytes: u64) -> Result<DropReservation, CliResponse> {
    let (max_files, max_bytes) = drop_limits();
    let Ok(root) = dunce::canonicalize(&Path::new(&resolved.path)) else {
        return Err(CliResponse::internal_error("workspace root unavailable"));
    };
    let key = format!("{}\u{0}{}", resolved.project_id, d.user);
    let turn = {
        let (m, c) = drop_busy();
        let mut busy = m.lock().unwrap_or_else(|p| p.into_inner());
        while busy.contains(&key) {
            busy = c.wait(busy).unwrap_or_else(|p| p.into_inner());
        }
        busy.insert(key.clone());
        DropReservation { key }
    };
    let (mut files, mut bytes) = (0u64, 0u64);
    let mut seen: Vec<PathBuf> = Vec::new();
    for f in d.roots.write.iter().chain(d.roots.upload.iter()) {
        let mine = root.join(f).join(&d.user);
        let Ok(canon) = dunce::canonicalize(&mine) else { continue };
        if !canon.starts_with(&root) || seen.iter().any(|s| canon.starts_with(s) || s.starts_with(&canon)) {
            continue;
        }
        tally(&canon, &mut files, &mut bytes, max_files);
        seen.push(canon);
    }
    if files + add_files > max_files || bytes.saturating_add(add_bytes) > max_bytes {
        return Err(CliResponse {
            status: "413 Payload Too Large",
            content_type: "application/json",
            body: serde_json::json!({
                "error": "too_large",
                "message": format!(
                    "your drop folders in this room are full: {files} files, {bytes} bytes (at most {max_files} files and {max_bytes} bytes); ask the owner to clear them"
                ),
            })
            .to_string(),
        });
    }
    Ok(turn)
}

fn skin_json_workspace(v: &serde_json::Value) -> Result<String, CliResponse> {
    let workspace = v
        .get("workspace")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if workspace.is_empty() {
        Err(CliResponse::bad_request("missing workspace"))
    } else {
        Ok(workspace)
    }
}

fn handle_skin_read_binary(params: &HashMap<String, String>, pass: &SkinPass) -> CliResponse {
    let ws = match need_workspace(params) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let resolved = match resolve_skin_workspace(pass, &ws) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if !pass.has_cap_in_room(&resolved.project_id, crate::skin_routes::FILES_READ) {
        return crate::skin_routes::missing_cap_response(crate::skin_routes::FILES_READ);
    }
    let rel = match need_rel_path(params) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let jailed = match skin_jail_read(pass, &resolved, &rel) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let abs = jailed.to_string_lossy().to_string();
    match fsc::read_binary_file(&abs) {
        Ok(bytes) => {
            let b64 = B64.encode(&bytes);
            CliResponse::ok_json(serde_json::json!({ "base64": b64 }).to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

fn handle_skin_read_range(params: &HashMap<String, String>, pass: &SkinPass) -> CliResponse {
    let ws = match need_workspace(params) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let resolved = match resolve_skin_workspace(pass, &ws) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if !pass.has_cap_in_room(&resolved.project_id, crate::skin_routes::FILES_READ) {
        return crate::skin_routes::missing_cap_response(crate::skin_routes::FILES_READ);
    }
    let rel = match need_rel_path(params) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let jailed = match skin_jail_read(pass, &resolved, &rel) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let abs = jailed.to_string_lossy().to_string();
    let offset: u64 = match params.get("offset").map(|v| v.parse()) {
        Some(Ok(v)) => v,
        Some(Err(_)) => return CliResponse::bad_request("invalid 'offset' parameter"),
        None => 0,
    };
    let len: u64 = match params.get("len").map(|v| v.parse()) {
        Some(Ok(v)) => v,
        Some(Err(_)) => return CliResponse::bad_request("invalid 'len' parameter"),
        None => 8 * 1024 * 1024,
    };
    match fsc::read_file_range(&abs, offset, len) {
        Ok((bytes, size)) => {
            let eof = offset.saturating_add(bytes.len() as u64) >= size;
            CliResponse::ok_json(
                serde_json::json!({
                    "base64": B64.encode(&bytes),
                    "len": bytes.len() as u64,
                    "size": size,
                    "eof": eof,
                })
                .to_string(),
            )
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

fn handle_skin_upload_binary(body: &[u8], pass: &SkinPass) -> CliResponse {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let workspace = match skin_json_workspace(&v) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let dir_sent = v
        .get("dir")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let filename = v
        .get("filename")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if filename.is_empty() {
        return CliResponse::bad_request("Missing 'filename' parameter");
    }
    let b64 = match v.get("base64").and_then(|x| x.as_str()) {
        Some(s) => s,
        None => return CliResponse::bad_request("Missing 'base64' parameter"),
    };
    let bytes = match B64.decode(b64.as_bytes()) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid base64: {e}")),
    };
    let (resolved, mode) = match skin_resolve_upload(pass, &workspace) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let UploadTarget { scope, dir, need, drop: drop_target } = match upload_target(pass, &resolved, &mode, dir_sent) {
        Ok(t) => t,
        Err(r) => return r,
    };
    // Round 2 A4: the drop's reservation lives until this request ends.
    let mut _reserved: Option<DropReservation> = None;
    if let Some(r) = scope_lexical(&scope, &dir, need) {
        return r;
    }
    let jailed_dir = match jail_rel_path(&resolved.path, &dir, true) {
        Ok(p) => p,
        Err(e) => return CliResponse::bad_request(e),
    };
    if let Some(r) = refuse_protected_upload(pass, &scope, &resolved, &dir, &jailed_dir, true, need) {
        return r;
    }
    if let Some(r) = refuse_protected_upload(
        pass,
        &scope,
        &resolved,
        &format!("{dir}/{filename}"),
        &jailed_dir.join(&filename),
        true,
        need,
    ) {
        return r;
    }
    let abs_dir = match &mode {
        UploadMode::Write => jailed_dir.to_string_lossy().to_string(),
        // An upload-only drop makes its own subfolder through the jail
        // (never through a symlink out), then checks where it landed.
        UploadMode::Drop { user } => {
            let Some(d) = &drop_target else { return CliResponse::internal_error("drop target missing") };
            match drop_reserve(&resolved, d, 1, bytes.len() as u64) {
                Ok(r) => _reserved = Some(r),
                Err(r) => return r,
            }
            if let Err(r) = ensure_user_dir(&resolved, &scope, d) {
                return r;
            }
            let root = match dunce::canonicalize(&Path::new(&resolved.path)) {
                Ok(r) => r,
                Err(e) => return CliResponse::internal_error(format!("workspace root unavailable: {e}")),
            };
            // Any mkdir failure (a file in the way, a link out) is the
            // plain missing-cap 403: no "not a folder" oracle (review A1).
            let canon = match jail_mkdir_p(&root, &dir) {
                Ok(d) => d,
                Err(e) => {
                    k2_core::log_debug!("[fs/upload-binary] drop mkdir refused for {user}: {e}");
                    return scope_refused(PathNeed::Upload);
                }
            };
            if let Some(r) = refuse_protected_upload(pass, &scope, &resolved, &dir, &canon, true, need) {
                return r;
            }
            canon.to_string_lossy().to_string()
        }
    };
    match fsc::write_upload(&abs_dir, &filename, &bytes) {
        Ok(path) => {
            let root = dunce::canonicalize(&Path::new(&resolved.path))
                .unwrap_or_else(|_| PathBuf::from(&resolved.path));
            let abs = path.to_string_lossy().to_string();
            crate::session_events::emit_fs_changed_for_paths([abs.clone()]);
            let rel = relative_under(&root, &abs);
            // AC10: `size` is additive for both caps.
            CliResponse::ok_json(serde_json::json!({ "path": rel, "size": bytes.len() as u64 }).to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// The folder an upload lands in: the one the caller sent, else the room
/// line's `uploadDir` (R3), else the old 400.
fn upload_dir_or_default(sent: String, scope: &RoomPathScope) -> Result<String, CliResponse> {
    if !sent.is_empty() {
        return Ok(sent);
    }
    match scope.upload_dir.as_deref() {
        Some(d) => Ok(d.to_string()),
        None => Err(CliResponse::bad_request("Destination directory is empty")),
    }
}

/// Jailed `mkdir -p` for a guest write: walks `rel` from the canonical
/// room `root` one component at a time, creating missing folders. An
/// existing symlinked component is followed only when it resolves to a
/// folder inside the room, so nothing is ever created outside the jail
/// (plain `create_dir_all` would create through a symlink that points
/// out). `rel` must already have passed [`jail_rel_path`]. Returns the
/// canonical folder.
fn jail_mkdir_p(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let mut cur = root.to_path_buf();
    for comp in Path::new(rel.trim()).components() {
        let name = match comp {
            Component::CurDir => continue,
            Component::Normal(n) => n,
            _ => return Err("path must not contain '..'".to_string()),
        };
        let next = cur.join(name);
        match std::fs::symlink_metadata(&next) {
            Ok(m) if m.file_type().is_dir() => cur = next,
            Ok(m) if m.file_type().is_symlink() => {
                let canon = dunce::canonicalize(&next)
                    .map_err(|e| format!("Invalid path: {e}"))?;
                if !canon.starts_with(root) {
                    return Err("path escapes the workspace".to_string());
                }
                if !canon.is_dir() {
                    return Err(format!("not a folder: {rel}"));
                }
                cur = canon;
            }
            Ok(_) => return Err(format!("not a folder: {rel}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::create_dir(&next) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(format!("Failed to create destination dir: {e}")),
                }
                // Re-check what is there now (a racing create could be a
                // symlink): it must be a real folder inside the room.
                let canon = dunce::canonicalize(&next)
                    .map_err(|e| format!("Invalid path: {e}"))?;
                if !canon.starts_with(root) || !canon.is_dir() {
                    return Err("path escapes the workspace".to_string());
                }
                cur = canon;
            }
            Err(e) => return Err(format!("Invalid path: {e}")),
        }
    }
    let canon = dunce::canonicalize(&cur).map_err(|e| format!("Invalid path: {e}"))?;
    if !canon.starts_with(root) {
        return Err("path escapes the workspace".to_string());
    }
    Ok(canon)
}

/// Stable identity a guest upload is bound to: the login principal (so a
/// re-login can resume), else the platform pass id.
pub(crate) fn guest_upload_principal(pass: &SkinPass) -> String {
    match pass.principal_id.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => format!("principal:{p}"),
        None => format!("pass:{}", pass.id),
    }
}

fn chunk_status(status: &'static str, body: serde_json::Value) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: body.to_string(),
    }
}

fn guest_chunk_error(e: k2_core::fs_guest_upload::GuestChunkError) -> CliResponse {
    use k2_core::fs_guest_upload::GuestChunkError as E;
    match e {
        E::OffsetMismatch { expected } => chunk_status(
            "409 Conflict",
            serde_json::json!({
                "error": "offset_mismatch",
                "expected": expected,
                "message": format!("resume from offset {expected}"),
            }),
        ),
        E::TooLarge(m) => chunk_status(
            "413 Payload Too Large",
            serde_json::json!({ "error": "too_large", "message": m }),
        ),
        E::NoSpace(m) => chunk_status(
            "507 Insufficient Storage",
            serde_json::json!({ "error": "no_space", "message": m }),
        ),
        E::Busy => chunk_status(
            "409 Conflict",
            serde_json::json!({
                "error": "upload_busy",
                "message": "another chunk of this upload is still being written; ask status, then continue",
            }),
        ),
        E::Invalid(m) => CliResponse::bad_request(m),
        E::Io(m) => CliResponse::internal_error(m),
    }
}

/// App guest form of `POST /cli/fs/upload-chunk` (resumable large upload).
///
/// Body (JSON): `workspace` (room handle), `dir` (room-relative folder,
/// `.` = room root; created when missing), `filename`, `upload_id`
/// (client-chosen, at most 200 characters), and either
/// - `status: true` → `{received, done:false}`: bytes already held for
///   this guest + dir + filename + upload_id (0 when none), or
/// - `offset`, `base64` (one chunk, at most 16 MiB decoded), `is_last`,
///   optional `total_bytes` on the offset-0 chunk.
///
/// Needs `files:write` in that room, or `files:upload` (an upload-only
/// drop: the file lands in `<drop folder>/<username>/`, see
/// [`upload_target`]). The temp part lives in the
/// destination folder under a dot-name bound to the guest
/// (`fs_guest_upload::guest_part_name`); the final chunk publishes it
/// under `filename`, or `name (1).ext` when taken, never replacing a file.
/// Responses: 200 `{received, done:false}` / `{path, done:true, size}`
/// (`path` room-relative); 409 `{error:"offset_mismatch", expected}` or
/// `{error:"upload_busy"}` (a retry racing its original); 413 chunk
/// or transfer too large; 507 not enough disk; 400 bad input or a path
/// outside the room; 403 room/cap.
fn handle_skin_upload_chunk(body: &[u8], pass: &SkinPass) -> CliResponse {
    use k2_core::fs_guest_upload as gu;
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let workspace = match skin_json_workspace(&v) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let text = |key: &str| {
        v.get(key)
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let dir_sent = text("dir");
    let filename = text("filename");
    if filename.is_empty() {
        return CliResponse::bad_request("Missing 'filename' parameter");
    }
    let upload_id = text("upload_id");
    if upload_id.is_empty() {
        return CliResponse::bad_request("Missing 'upload_id' parameter");
    }
    if upload_id.chars().count() > gu::MAX_UPLOAD_ID_LEN {
        return CliResponse::bad_request(format!(
            "'upload_id' is longer than {} characters",
            gu::MAX_UPLOAD_ID_LEN
        ));
    }
    let status_only = v.get("status").and_then(|x| x.as_bool()).unwrap_or(false);
    // AC10: files:write (today's rules) or files:upload (a drop into the
    // uploader's own subfolder, `upload_target`).
    let (resolved, mode) = match skin_resolve_upload(pass, &workspace) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let UploadTarget { scope, dir, need, drop: drop_target } = match upload_target(pass, &resolved, &mode, dir_sent) {
        Ok(t) => t,
        Err(r) => return r,
    };
    // Round 2 A4: the drop's reservation lives until this request ends.
    let mut _reserved: Option<DropReservation> = None;
    if let Some(r) = scope_lexical(&scope, &dir, need) {
        return r;
    }
    let jailed_dir = match jail_rel_path(&resolved.path, &dir, true) {
        Ok(p) => p,
        Err(e) => return CliResponse::bad_request(e),
    };
    if let Some(r) = refuse_protected_upload(pass, &scope, &resolved, &dir, &jailed_dir, true, need) {
        return r;
    }
    let name = gu::final_name(&filename);
    // The published name never writes through an existing entry (see
    // `refuse_protected_upload`), so the leaf is not followed here.
    if let Some(r) = refuse_protected_upload(
        pass,
        &scope,
        &resolved,
        &format!("{dir}/{name}"),
        &jailed_dir.join(&name),
        false,
        need,
    ) {
        return r;
    }
    let root = match dunce::canonicalize(&Path::new(&resolved.path)) {
        Ok(r) => r,
        Err(e) => return CliResponse::internal_error(format!("workspace root unavailable: {e}")),
    };
    let principal = guest_upload_principal(pass);

    if status_only {
        let canon = match dunce::canonicalize(&jailed_dir) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return CliResponse::ok_json(
                    serde_json::json!({ "received": 0, "done": false }).to_string(),
                );
            }
            Err(e) => return CliResponse::bad_request(format!("Invalid path: {e}")),
        };
        if !canon.starts_with(&root) || !canon.is_dir() {
            return CliResponse::bad_request("path escapes the workspace");
        }
        let part = gu::guest_part_name(&principal, &resolved.project_id, &canon, &name, &upload_id);
        return match gu::guest_part_len(&canon, &part) {
            Ok(n) => CliResponse::ok_json(
                serde_json::json!({ "received": n, "done": false }).to_string(),
            ),
            Err(e) => guest_chunk_error(e),
        };
    }

    let Some(offset) = v.get("offset").and_then(|x| x.as_u64()) else {
        return CliResponse::bad_request("Missing 'offset' parameter");
    };
    let Some(b64) = v.get("base64").and_then(|x| x.as_str()) else {
        return CliResponse::bad_request("Missing 'base64' parameter");
    };
    let is_last = v.get("is_last").and_then(|x| x.as_bool()).unwrap_or(false);
    let total_bytes = v.get("total_bytes").and_then(|x| x.as_u64());
    let bytes = match B64.decode(b64.as_bytes()) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid base64: {e}")),
    };
    if bytes.len() as u64 > fsc::MAX_UPLOAD_CHUNK_SIZE {
        return guest_chunk_error(gu::GuestChunkError::TooLarge(format!(
            "upload chunk too large ({} bytes > {} max)",
            bytes.len(),
            fsc::MAX_UPLOAD_CHUNK_SIZE
        )));
    }
    if let Some(d) = &drop_target {
        // Review A4: the first chunk counts a new file and its declared
        // size; every chunk counts its own bytes (reserved until this
        // request ends, round 2).
        let (add_files, add_bytes) = if offset == 0 { (1, total_bytes.unwrap_or(bytes.len() as u64)) } else { (0, bytes.len() as u64) };
        match drop_reserve(&resolved, d, add_files, add_bytes) {
            Ok(r) => _reserved = Some(r),
            Err(r) => return r,
        }
        if let Err(r) = ensure_user_dir(&resolved, &scope, d) {
            return r;
        }
    }
    let canon_dir = match jail_mkdir_p(&root, &dir) {
        Ok(d) => d,
        // An upload-only drop gets no "not a folder" oracle (review A1).
        Err(e) if matches!(mode, UploadMode::Drop { .. }) => {
            k2_core::log_debug!("[fs/upload-chunk] drop mkdir refused: {e}");
            return scope_refused(PathNeed::Upload);
        }
        Err(e) => return CliResponse::bad_request(e),
    };
    if let Some(r) = refuse_protected_upload(pass, &scope, &resolved, &dir, &canon_dir, true, need) {
        return r;
    }
    let part = gu::guest_part_name(&principal, &resolved.project_id, &canon_dir, &name, &upload_id);
    let part_abs = canon_dir.join(&part);
    let ledger = gu::ledger_dir();
    if offset == 0 {
        if let Err(e) = gu::ledger_record(&ledger, &part_abs) {
            return CliResponse::internal_error(e);
        }
    }
    match gu::write_guest_upload_chunk(&canon_dir, &part, &name, offset, &bytes, is_last, total_bytes) {
        Ok(gu::GuestChunkOutcome::Received(n)) => CliResponse::ok_json(
            serde_json::json!({ "received": n, "done": false }).to_string(),
        ),
        Ok(gu::GuestChunkOutcome::Done(path)) => {
            if std::fs::symlink_metadata(&part_abs).is_err() {
                if let Err(e) = gu::ledger_clear(&ledger, &part_abs) {
                    k2_core::log_debug!("[fs/upload-chunk] {e}");
                }
            }
            let abs = path.to_string_lossy().to_string();
            crate::session_events::emit_fs_changed_for_paths([abs.clone()]);
            let rel = relative_under(&root, &abs);
            // AC10: `size` (the landed file's bytes) is additive.
            let size = match std::fs::symlink_metadata(&path) {
                Ok(m) => m.len(),
                Err(e) => return CliResponse::internal_error(format!("landed upload stat: {e}")),
            };
            CliResponse::ok_json(serde_json::json!({ "path": rel, "done": true, "size": size }).to_string())
        }
        Err(e) => guest_chunk_error(e),
    }
}

/// May `pass` still send bytes into `rel_dir` of room `project_id`? The
/// door asks on every range of a resumable upload (`transfer_door`), so a
/// cap, a room or a folder taken away stops the upload at its next request.
/// `drop`: the session was made with `files:upload` alone (an upload-only
/// drop), which `files:write` also covers. The lexical scope only: the
/// landed checks ran when the session was made, and its folder is fixed.
pub(crate) fn upload_still_allowed(pass: &SkinPass, project_id: &str, rel_dir: &str, drop: bool) -> bool {
    let Ok(resolved) = resolve_skin_workspace(pass, project_id) else { return false };
    let write = pass.has_cap_in_room(&resolved.project_id, crate::skin_routes::FILES_WRITE);
    if write {
        if let Ok(scope) = room_scope(pass, &resolved, PathNeed::Write) {
            if scope_lexical(&scope, rel_dir, PathNeed::Write).is_none() {
                return true;
            }
        }
    }
    if !drop || !pass.has_cap_in_room(&resolved.project_id, k2_core::skin::CAP_FILES_UPLOAD) {
        return false;
    }
    let Ok(line) = room_scope(pass, &resolved, PathNeed::Upload) else { return false };
    let Some(scope) = drop_scope(&line) else { return false };
    scope_lexical(&scope, rel_dir, PathNeed::Upload).is_none()
}

/// `POST /cli/fs/upload/create` (`files.uploadCreate`, 0.46.9): a
/// resumable upload session for an OS background transfer
/// (`k2_core::fs_transfer`; the bytes go to `transfer_door`).
///
/// Body: `workspace`, `dir` (room-relative; else the line's `uploadDir`),
/// `filename`, `size` (bytes), optional `sha256` (hex of the whole file)
/// and `tokenTtlSecs` (60..=86400, default 6 h). Every rule of
/// `files.uploadChunk` applies here, once: `files:write`, or `files:upload`
/// (the drop lands in `<drop folder>/<username>/` and its caps count this
/// file's whole size plus the person's other unfinished uploads there), the
/// folder scope, `.k2/` and heartbeat ground, the 10 GiB ceiling, free disk
/// and the disk guard. Only a pass K2 can read again by its id may make one
/// (a guest's session or a platform pass, never the App's own link pass).
///
/// 200 `{uploadId, transferToken, tokenExpiresAt, url, size, partMax, received,
/// missing, receivedBytes, complete, done, expiresAt, dir}`; 400 bad input
/// or `guest_pass_required`; 403 room/cap; 413 over the ceiling or a drop
/// cap; 429 `too_many_uploads`; 507 no space.
pub fn handle_skin_upload_create(body: &[u8], pass: &SkinPass) -> CliResponse {
    use k2_core::fs_transfer as ft;
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let workspace = match skin_json_workspace(&v) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let text = |key: &str| v.get(key).and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    let dir_sent = text("dir");
    let filename = text("filename");
    if filename.is_empty() {
        return CliResponse::bad_request("Missing 'filename' parameter");
    }
    let Some(size) = v.get("size").and_then(|x| x.as_u64()) else {
        return CliResponse::bad_request("Missing 'size' parameter (the file's bytes)");
    };
    let sha256 = match ft::parse_sha256(v.get("sha256").and_then(|x| x.as_str())) {
        Ok(s) => s,
        Err(e) => return CliResponse::bad_request(e),
    };
    let ttl_asked = match v.get("tokenTtlSecs") {
        None | Some(serde_json::Value::Null) => None,
        Some(x) => match x.as_u64() {
            Some(n) => Some(n),
            None => return CliResponse::bad_request("'tokenTtlSecs' is a number of seconds"),
        },
    };
    if size > fsc::MAX_TRANSFER_SIZE {
        return crate::transfer_routes::error_response(&ft::TransferError::TooLarge(format!(
            "transfer of {size} bytes exceeds the {} GiB ceiling",
            fsc::MAX_TRANSFER_SIZE / (1024 * 1024 * 1024)
        )));
    }
    // The door re-reads this pass by id on every range: one K2 can't (the
    // App's own link pass) can't hold an upload.
    match k2_core::skin::resolve_skin_token_id(&pass.id) {
        Ok(Some(_)) => {}
        Ok(None) => return crate::transfer_routes::guest_pass_required(),
        Err(e) => return CliResponse::internal_error(e),
    }
    let (resolved, mode) = match skin_resolve_upload(pass, &workspace) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let UploadTarget { scope, dir, need, drop: drop_target } = match upload_target(pass, &resolved, &mode, dir_sent) {
        Ok(t) => t,
        Err(r) => return r,
    };
    let mut _reserved: Option<DropReservation> = None;
    if let Some(r) = scope_lexical(&scope, &dir, need) {
        return r;
    }
    let jailed_dir = match jail_rel_path(&resolved.path, &dir, true) {
        Ok(p) => p,
        Err(e) => return CliResponse::bad_request(e),
    };
    if let Some(r) = refuse_protected_upload(pass, &scope, &resolved, &dir, &jailed_dir, true, need) {
        return r;
    }
    let name = k2_core::fs_guest_upload::final_name(&filename);
    if let Some(r) = refuse_protected_upload(pass, &scope, &resolved, &format!("{dir}/{name}"), &jailed_dir.join(&name), false, need) {
        return r;
    }
    let root = match dunce::canonicalize(Path::new(&resolved.path)) {
        Ok(r) => r,
        Err(e) => return CliResponse::internal_error(format!("workspace root unavailable: {e}")),
    };
    let principal = guest_upload_principal(pass);
    let now = ft::now_secs();
    if let Some(d) = &drop_target {
        // The drop caps count this whole file now, plus what this person's
        // other unfinished uploads in this room still have to send.
        let pending: u64 = match ft::live_uploads_of(&ft::uploads_dir(), &principal, now) {
            Ok(live) => live
                .iter()
                .filter(|s| s.done.is_none() && s.drop && s.project_id == resolved.project_id)
                .map(|s| {
                    let held = std::fs::symlink_metadata(s.dir.join(&s.part)).map(|m| m.len()).unwrap_or(0);
                    s.size.saturating_sub(held)
                })
                .sum(),
            Err(e) => return CliResponse::internal_error(e.to_string()),
        };
        match drop_reserve(&resolved, d, 1, size.saturating_add(pending)) {
            Ok(r) => _reserved = Some(r),
            Err(r) => return r,
        }
        if let Err(r) = ensure_user_dir(&resolved, &scope, d) {
            return r;
        }
    }
    let canon_dir = match jail_mkdir_p(&root, &dir) {
        Ok(d) => d,
        Err(e) if matches!(mode, UploadMode::Drop { .. }) => {
            k2_core::log_debug!("[fs/upload/create] drop mkdir refused: {e}");
            return scope_refused(PathNeed::Upload);
        }
        Err(e) => return CliResponse::bad_request(e),
    };
    if let Some(r) = refuse_protected_upload(pass, &scope, &resolved, &dir, &canon_dir, true, need) {
        return r;
    }
    let rel_dir = relative_under(&root, &canon_dir.to_string_lossy());
    let ttl = ft::clamp_ttl(ttl_asked, ft::TOKEN_TTL_DEFAULT_SECS);
    let new = ft::NewUpload {
        holder: ft::Holder { principal, pass_id: Some(pass.id.clone()), username: pass.username.clone() },
        project_id: resolved.project_id.clone(),
        root,
        dir: canon_dir,
        rel_dir: rel_dir.clone(),
        filename: name,
        size,
        sha256,
        drop: matches!(mode, UploadMode::Drop { .. }),
        token_ttl_secs: ttl,
    };
    match ft::create_upload(&ft::uploads_dir(), &k2_core::fs_guest_upload::ledger_dir(), new, now) {
        Ok((s, token)) => {
            crate::transfer_door::note_live();
            crate::thread_attachments::audit(
                "files.upload_create",
                &format!("guest:{}", pass.username.trim()),
                format!("room={} dir={rel_dir:?} size={size} id={}", s.project_id, s.id),
            );
            let mut out = s.status_json();
            out["transferToken"] = serde_json::json!(token);
            out["tokenExpiresAt"] = serde_json::json!(s.tokens[0].expires_at.saturating_mul(1000));
            out["dir"] = serde_json::json!(rel_dir);
            CliResponse::ok_json(out.to_string())
        }
        Err(e) => crate::transfer_routes::error_response(&e),
    }
}

fn handle_skin_create(body: &[u8], pass: &SkinPass) -> CliResponse {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let workspace = match skin_json_workspace(&v) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let rel = v
        .get("path")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if rel.is_empty() {
        return CliResponse::bad_request("Missing 'path' parameter");
    }
    let is_directory = v
        .get("is_directory")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    let resolved = match skin_resolve_write(pass, &workspace) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let scope = match scope_gate(pass, &resolved, &rel, PathNeed::Write) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let jailed = match jail_rel_path(&resolved.path, &rel, true) {
        Ok(p) => p,
        Err(e) => return CliResponse::bad_request(e),
    };
    if let Some(r) = refuse_protected_write(pass, &scope, &resolved, &rel, &jailed, false) {
        return r;
    }
    let abs = jailed.to_string_lossy().to_string();
    match fsc::create_entry(&abs, is_directory) {
        Ok(()) => {
            crate::session_events::emit_fs_changed_for_paths([abs]);
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

fn handle_skin_copy_or_move(body: &[u8], pass: &SkinPass, is_move: bool) -> CliResponse {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let workspace = match skin_json_workspace(&v) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let sources: Vec<String> = match v.get("sources").and_then(|x| x.as_array()) {
        Some(arr) => arr
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect(),
        None => return CliResponse::bad_request("Missing 'sources' parameter"),
    };
    if sources.is_empty() {
        return CliResponse::bad_request("Missing 'sources' parameter");
    }
    let dest = v
        .get("destination")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if dest.is_empty() {
        return CliResponse::bad_request("Missing 'destination' parameter");
    }
    let resolved = match skin_resolve_write(pass, &workspace) {
        Ok(r) => r,
        Err(e) => return e,
    };
    // R3, before any disk access: a move takes its sources away (write
    // folders only); a copy reads them (read or write folders). The
    // destination is always a write folder.
    let source_need = if is_move { PathNeed::Write } else { PathNeed::Read };
    let scope = match room_scope(pass, &resolved, source_need) {
        Ok(s) => s,
        Err(r) => return r,
    };
    for rel in &sources {
        if let Some(r) = scope_lexical(&scope, rel, source_need) {
            return r;
        }
    }
    if let Some(r) = scope_lexical(&scope, &dest, PathNeed::Write) {
        return r;
    }
    let mut abs_sources = Vec::with_capacity(sources.len());
    for rel in &sources {
        match jail_rel_path(&resolved.path, rel, false) {
            Ok(p) => {
                // 0.46.16: never carry a hidden path out from under its
                // pattern (a folder that holds one stays where it is).
                if source_holds_hidden(&scope, &resolved, rel, &p) {
                    return scope_refused(source_need);
                }
                // R2: a move carries the source away, so `.` / `.k2` count.
                if is_move {
                    if let Some(r) = refuse_protected_write(pass, &scope, &resolved, rel, &p, true) {
                        return r;
                    }
                } else if let Some(r) = scope_gate_landed(&scope, &resolved, &p, PathNeed::Read, true) {
                    return r;
                }
                abs_sources.push(p.to_string_lossy().to_string())
            }
            Err(e) => return CliResponse::bad_request(e),
        }
    }
    let dest_path = match jail_rel_path(&resolved.path, &dest, true) {
        Ok(p) => p,
        Err(e) => return CliResponse::bad_request(e),
    };
    if let Some(r) = refuse_protected_write(pass, &scope, &resolved, &dest, &dest_path, false) {
        return r;
    }
    // The landed path is `<dest>/<source name>` when dest is a folder.
    for abs in abs_sources.iter() {
        let Some(leaf) = Path::new(abs).file_name() else { continue };
        let leaf_s = leaf.to_string_lossy();
        if let Some(r) = refuse_protected_write(
            pass,
            &scope,
            &resolved,
            &format!("{dest}/{leaf_s}"),
            &dest_path.join(leaf),
            false,
        ) {
            return r;
        }
    }
    let abs_dest = dest_path.to_string_lossy().to_string();
    let result = if is_move {
        fsc::move_files(&abs_sources, &abs_dest)
    } else {
        fsc::copy_files(&abs_sources, &abs_dest)
    };
    match result {
        Ok(()) => {
            if is_move {
                let mut paths = abs_sources;
                paths.push(abs_dest);
                crate::session_events::emit_fs_changed_for_paths(paths);
            } else {
                crate::session_events::emit_fs_changed_for_paths([abs_dest]);
            }
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

fn handle_skin_copy(body: &[u8], pass: &SkinPass) -> CliResponse {
    handle_skin_copy_or_move(body, pass, false)
}

fn handle_skin_move(body: &[u8], pass: &SkinPass) -> CliResponse {
    handle_skin_copy_or_move(body, pass, true)
}

/// Skin `new_name` is one [`Component::Normal`]. `.` and `..` are not.
/// Owner [`fsc::rename`] still allows those two.
fn skin_new_name_is_normal_segment(name: &str) -> bool {
    let mut comps = Path::new(name).components();
    match comps.next() {
        Some(Component::Normal(_)) => comps.next().is_none(),
        _ => false,
    }
}

fn relative_inside(root: &str, abs: &str) -> Result<String, String> {
    let root = dunce::canonicalize(&Path::new(root))
        .map_err(|e| format!("workspace root unavailable: {e}"))?;
    let abs_path = Path::new(abs);
    let canon = dunce::canonicalize(&abs_path).unwrap_or_else(|_| abs_path.to_path_buf());
    let rel = canon
        .strip_prefix(&root)
        .or_else(|_| abs_path.strip_prefix(&root))
        .map_err(|_| "renamed path left the workspace".to_string())?;
    let s = rel.to_string_lossy().replace('\\', "/");
    if s.is_empty() || s.split('/').any(|p| p == "..") {
        return Err("renamed path left the workspace".to_string());
    }
    Ok(s)
}

fn handle_skin_rename(body: &[u8], pass: &SkinPass) -> CliResponse {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let workspace = match skin_json_workspace(&v) {
        Ok(w) => w,
        Err(e) => return e,
    };
    let rel = v
        .get("path")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if rel.is_empty() {
        return CliResponse::bad_request("Missing 'path' parameter");
    }
    let new_name = v
        .get("new_name")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if new_name.is_empty() {
        return CliResponse::bad_request("Missing 'new_name' parameter");
    }
    let resolved = match skin_resolve_write(pass, &workspace) {
        Ok(r) => r,
        Err(e) => return e,
    };
    // Reject before rename. Owner `fs_commands::rename` allows `.` and `..`.
    if !skin_new_name_is_normal_segment(&new_name) {
        return CliResponse::bad_request("new_name must be a single path segment");
    }
    // R3 before any disk access: the source and the new name both sit in
    // a write folder.
    let parent_rel = Path::new(&rel)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let new_rel = if parent_rel.is_empty() { new_name.clone() } else { format!("{parent_rel}/{new_name}") };
    let scope = match scope_gate(pass, &resolved, &rel, PathNeed::Write) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Some(r) = scope_lexical(&scope, &new_rel, PathNeed::Write) {
        return r;
    }
    let jailed = match jail_rel_path(&resolved.path, &rel, false) {
        Ok(p) => p,
        Err(e) => return CliResponse::bad_request(e),
    };
    // 0.46.16: renaming a folder that holds a rooted hidden path would
    // bring it out from under its pattern.
    if source_holds_hidden(&scope, &resolved, &rel, &jailed) {
        return scope_refused(PathNeed::Write);
    }
    // R2: neither the source nor the renamed path may be heartbeat ground.
    if let Some(r) = refuse_protected_write(pass, &scope, &resolved, &rel, &jailed, true) {
        return r;
    }
    if let Some(parent) = jailed.parent() {
        if let Some(r) = refuse_protected_write(
            pass,
            &scope,
            &resolved,
            &new_rel,
            &parent.join(&new_name),
            false,
        ) {
            return r;
        }
    }
    let abs = jailed.to_string_lossy().to_string();
    match fsc::rename(&abs, &new_name) {
        Ok(new_abs) => {
            crate::session_events::emit_fs_changed_for_paths([abs, new_abs.clone()]);
            match relative_inside(&resolved.path, &new_abs) {
                Ok(rel_out) => {
                    CliResponse::ok_json(serde_json::json!({ "path": rel_out }).to_string())
                }
                Err(e) => CliResponse::internal_error(e),
            }
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct MoveCopyBody {
    sources: Vec<String>,
    destination: String,
}

/// Runs `f` holding the writer lock of every board file named, in one order so
/// two moves can't wait on each other. A moved folder can't take the locks of
/// what's inside it: that's an outside change to a save in flight, which sees
/// `not_found` and answers it.
fn with_board_locks<T>(sources: &[String], f: impl FnOnce() -> T) -> T {
    fn hold<T>(boards: &[&String], f: impl FnOnce() -> T) -> T {
        match boards.split_first() {
            Some((p, rest)) => crate::board_file::save::with_writer_lock(Path::new(p.as_str()), || hold(rest, f)),
            None => f(),
        }
    }
    let mut boards: Vec<&String> = sources.iter().filter(|s| crate::docs::target::is_board_file(s)).collect();
    boards.sort();
    boards.dedup();
    hold(&boards, f)
}

pub fn handle_move(body: &[u8]) -> CliResponse {
    let parsed: MoveCopyBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match with_board_locks(&parsed.sources, || fsc::move_files(&parsed.sources, &parsed.destination)) {
        Ok(()) => {
            for s in parsed.sources.iter().filter(|s| crate::docs::target::is_board_file(s)) {
                if let Some(name) = Path::new(s).file_name() {
                    crate::board_file::held::retarget(Path::new(s), &Path::new(&parsed.destination).join(name));
                }
            }
            let mut paths = parsed.sources;
            paths.push(parsed.destination);
            crate::session_events::emit_fs_changed_for_paths(paths);
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_copy(body: &[u8]) -> CliResponse {
    let parsed: MoveCopyBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match fsc::copy_files(&parsed.sources, &parsed.destination) {
        Ok(()) => {
            // Destination is where new entries appear; sources are unchanged.
            crate::session_events::emit_fs_changed_for_paths([parsed.destination]);
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct DeleteBody {
    paths: Vec<String>,
    #[serde(default)]
    permanent: bool,
}

pub fn handle_delete(body: &[u8]) -> CliResponse {
    let parsed: DeleteBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match fsc::delete(&parsed.paths, parsed.permanent) {
        Ok(()) => {
            crate::session_events::emit_fs_changed_for_paths(parsed.paths);
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct RenameBody {
    old_path: String,
    new_name: String,
}

pub fn handle_rename(body: &[u8]) -> CliResponse {
    let parsed: RenameBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let renamed = if crate::docs::target::is_board_file(&parsed.old_path) {
        // A rename between a save's last compare and its replace would bring
        // back the old name with stale bytes.
        crate::board_file::save::with_writer_lock(Path::new(&parsed.old_path), || fsc::rename(&parsed.old_path, &parsed.new_name))
    } else {
        fsc::rename(&parsed.old_path, &parsed.new_name)
    };
    match renamed {
        Ok(new_path) => {
            if crate::docs::target::is_board_file(&parsed.old_path) {
                crate::board_file::held::retarget(Path::new(&parsed.old_path), Path::new(&new_path));
            }
            crate::session_events::emit_fs_changed_for_paths([parsed.old_path, new_path.clone()]);
            CliResponse::ok_json(serde_json::json!({ "path": new_path }).to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct CreateBody {
    path: String,
    #[serde(default)]
    is_directory: bool,
}

pub fn handle_create(body: &[u8]) -> CliResponse {
    let parsed: CreateBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match fsc::create_entry(&parsed.path, parsed.is_directory) {
        Ok(()) => {
            crate::session_events::emit_fs_changed_for_paths([parsed.path]);
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct UntitledNameBody {
    dir: String,
    #[serde(default)]
    ext: Option<String>,
    #[serde(default)]
    taken: Vec<String>,
}

fn untitled_answer(path: &std::path::Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    serde_json::json!({ "path": path.to_string_lossy(), "name": name }).to_string()
}

/// `POST /cli/fs/untitled-name` `{dir, ext?, taken?}` → `{path, name}`: the
/// next free `Untitled[-N][.ext]` in `dir`, checked against the disk and
/// `taken` (the caller's open tabs that aren't files yet). Writes nothing, so
/// it's only a suggestion: the write goes through `fs/create-untitled`. POST
/// because `taken` can outgrow a URL.
pub fn handle_untitled_name(body: &[u8]) -> CliResponse {
    let parsed: UntitledNameBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match fsc::free_untitled_name(&parsed.dir, parsed.ext.as_deref(), &parsed.taken) {
        Ok(path) => CliResponse::ok_json(untitled_answer(&path)),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct CreateUntitledBody {
    dir: String,
    #[serde(default)]
    ext: Option<String>,
    #[serde(default)]
    prefer: Option<String>,
    #[serde(default)]
    taken: Vec<String>,
    #[serde(default)]
    content: String,
}

/// `POST /cli/fs/create-untitled` `{dir, ext?, prefer?, taken?, content?}` →
/// `{path, name}`: atomically create a new file under `prefer` if it's free,
/// else the next free `Untitled[-N][.ext]`. Never overwrites; two racing
/// creates get two files, so read `name` back (it may not be what you asked
/// for). A new Board tab sends `{dir, ext: "k5board", content}`.
pub fn handle_create_untitled(body: &[u8]) -> CliResponse {
    let parsed: CreateUntitledBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match fsc::create_untitled(
        &parsed.dir,
        parsed.ext.as_deref(),
        parsed.prefer.as_deref(),
        &parsed.taken,
        parsed.content.as_bytes(),
    ) {
        Ok(path) => {
            crate::session_events::emit_fs_changed_for_paths([path.to_string_lossy().to_string()]);
            CliResponse::ok_json(untitled_answer(&path))
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct UploadBinaryBody {
    dir: String,
    filename: String,
    base64: String,
}

/// Decode a base64 payload and write it into `dir` under a sanitized,
/// collision-free name. The renderer (or any remote client) chooses the
/// destination `dir` — for the terminal-drop case it's
/// `<workspace>/.k2so/downloads`. Mirrors `handle_read_binary`'s base64
/// transport in reverse: there we encode bytes OUT, here we decode bytes IN.
///
/// Size cap (`MAX_UPLOAD_SIZE`, 100MB) and path-traversal / sanitize /
/// collision logic all live in `fsc::write_upload` so they're testable
/// without HTTP. We reject the oversize case on the DECODED length before
/// touching the disk.
pub fn handle_upload_binary(body: &[u8]) -> CliResponse {
    let parsed: UploadBinaryBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let bytes = match B64.decode(parsed.base64.as_bytes()) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid base64: {e}")),
    };
    match fsc::write_upload(&parsed.dir, &parsed.filename, &bytes) {
        Ok(path) => {
            crate::session_events::emit_fs_changed_for_paths([path.to_string_lossy().to_string()]);
            CliResponse::ok_json(serde_json::json!({ "path": path.to_string_lossy() }).to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct UploadChunkBody {
    /// Client-generated unique id for this transfer (keys the temp `.part`).
    upload_id: String,
    dir: String,
    filename: String,
    /// Decoded-byte offset this chunk starts at; MUST match the part's current
    /// length (0 starts/restarts). Enforced in `fsc::write_upload_chunk`.
    offset: u64,
    base64: String,
    /// `true` on the final chunk → finalize (atomic rename into place).
    is_last: bool,
    /// Full expected transfer size, sent with the offset-0 chunk (0.40.22).
    /// Lets the daemon reject over-ceiling / won't-fit-on-disk transfers up
    /// front; optional for wire-compat with pre-0.40.22 clients.
    #[serde(default)]
    total_bytes: Option<u64>,
}

/// Streaming counterpart to [`handle_upload_binary`]: decode ONE chunk and
/// append it to the in-progress `.part` file (see `fsc::write_upload_chunk`).
/// Memory stays bounded at a single chunk regardless of the total transfer
/// size, so large "Clone to" bundles (GH #3) move without the 100 MB
/// single-shot cap. The per-chunk size guard + ordered-append enforcement live
/// in core so they're unit-testable without HTTP. Returns `{ path, done:true }`
/// on the finalizing chunk, `{ received, done:false }` for intermediate ones.
pub fn handle_upload_chunk(body: &[u8]) -> CliResponse {
    let parsed: UploadChunkBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let bytes = match B64.decode(parsed.base64.as_bytes()) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid base64: {e}")),
    };
    match fsc::write_upload_chunk(
        &parsed.dir,
        &parsed.filename,
        &parsed.upload_id,
        parsed.offset,
        &bytes,
        parsed.is_last,
        parsed.total_bytes,
    ) {
        Ok(Some(path)) => {
            // Final chunk only — intermediate appends are not user-visible.
            crate::session_events::emit_fs_changed_for_paths([path.to_string_lossy().to_string()]);
            CliResponse::ok_json(
                serde_json::json!({ "path": path.to_string_lossy(), "done": true }).to_string(),
            )
        }
        Ok(None) => CliResponse::ok_json(
            serde_json::json!({ "received": parsed.offset + bytes.len() as u64, "done": false })
                .to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

// ── compress (server-side folder → zip, 0.40.22) ──────────────────────
//
// Zipping a big tree takes minutes, and every `/cli/*` response is a
// single buffered body — so compress is an ASYNC JOB, the same
// start-then-poll shape as the daemon self-update (`update_routes`):
// `POST fs/compress` returns a job_id immediately, a worker thread
// streams the zip, `GET fs/compress-status?job_id=` snapshots progress,
// `POST fs/compress-cancel` raises the worker's cooperative cancel flag.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};

// ── extract (server-side zip → folder; inverse of compress) ───────────
// Same async job shape as compress. Defined just below the compress
// block so the two stay visually paired.

/// A compress job's observable state; `status` snapshots it. The cancel
/// flag rides along (skipped in serialization) so cancel needs no second
/// registry.
#[derive(Clone, serde::Serialize)]
pub struct CompressJob {
    pub job_id: String,
    /// `running` → `done` | `failed`. Terminal states stay in the map so a
    /// poll that races completion still sees the outcome (jobs are removed
    /// only when a new job starts — see `insert_compress_job`).
    pub phase: &'static str,
    /// Entries written / entries planned (the denominator is exact — the
    /// worker plans the walk before writing).
    pub done: u64,
    pub total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zip_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip)]
    cancel: Arc<AtomicBool>,
}

fn compress_jobs() -> &'static Mutex<HashMap<String, CompressJob>> {
    static JOBS: OnceLock<Mutex<HashMap<String, CompressJob>>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Insert a fresh job, evicting finished ones so the map can't grow
/// unbounded across a long daemon lifetime (in-flight jobs are kept).
fn insert_compress_job(job: CompressJob) {
    let mut map = compress_jobs().lock().unwrap_or_else(|e| e.into_inner());
    map.retain(|_, j| j.phase == "running");
    map.insert(job.job_id.clone(), job);
}

fn update_compress_job<F: FnOnce(&mut CompressJob)>(job_id: &str, f: F) {
    if let Some(job) = compress_jobs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(job_id)
    {
        f(job);
    }
}

fn get_compress_job(job_id: &str) -> Option<CompressJob> {
    compress_jobs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(job_id)
        .cloned()
}

/// Short collision-resistant job id (random hex via getrandom — same
/// scheme as `update_routes::new_job_id`).
fn new_compress_job_id() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::getrandom(&mut bytes).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        return format!("zip-{nanos:x}");
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Deserialize)]
struct CompressBody {
    path: String,
}

/// Start a compress job for the folder (or file) at `path`. Validation
/// that would make the job fail instantly (missing path) happens HERE so
/// the caller gets a 400 instead of a job that flips to `failed`; the
/// deep work streams on a worker thread. Returns `{ job_id }`.
pub fn handle_compress(body: &[u8]) -> CliResponse {
    let parsed: CompressBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if let Err(e) = k2_core::fs_commands::validate_path(&parsed.path) {
        return CliResponse::bad_request(e);
    }

    let job_id = new_compress_job_id();
    let cancel = Arc::new(AtomicBool::new(false));
    insert_compress_job(CompressJob {
        job_id: job_id.clone(),
        phase: "running",
        done: 0,
        total: 0,
        zip_path: None,
        error: None,
        cancel: cancel.clone(),
    });

    let src = parsed.path;
    let worker_job_id = job_id.clone();
    std::thread::spawn(move || {
        let progress_id = worker_job_id.clone();
        let result = k2_core::fs_compress::compress_to_zip(
            &src,
            &move |done, total| {
                update_compress_job(&progress_id, |j| {
                    j.done = done;
                    j.total = total;
                });
            },
            &cancel,
        );
        match result {
            Ok(path) => {
                let zip = path.to_string_lossy().to_string();
                update_compress_job(&worker_job_id, |j| {
                    j.phase = "done";
                    j.zip_path = Some(zip.clone());
                });
                // Zip lands next to the source — refresh both.
                crate::session_events::emit_fs_changed_for_paths([src, zip]);
            }
            Err(e) => {
                k2_core::log_debug!("[daemon] fs/compress — job {worker_job_id} FAILED: {e}");
                update_compress_job(&worker_job_id, |j| {
                    j.phase = "failed";
                    j.error = Some(e);
                });
            }
        }
    });

    CliResponse::ok_json(serde_json::json!({ "job_id": job_id }).to_string())
}

/// Snapshot a compress job (GET, `?job_id=`).
pub fn handle_compress_status(params: &HashMap<String, String>) -> CliResponse {
    let job_id = match params.get("job_id") {
        Some(id) if !id.is_empty() => id,
        _ => return CliResponse::bad_request("Missing 'job_id' parameter"),
    };
    match get_compress_job(job_id) {
        Some(job) => {
            CliResponse::ok_json(serde_json::to_string(&job).unwrap_or_else(|_| "{}".to_string()))
        }
        None => CliResponse::bad_request(format!("unknown job_id: {job_id}")),
    }
}

#[derive(Deserialize)]
struct CompressCancelBody {
    job_id: String,
}

/// Raise a running job's cooperative cancel flag. The worker notices
/// between entries, removes its `.part`, and flips the job to `failed`
/// ("Compression cancelled") — the status poll surfaces that terminally.
pub fn handle_compress_cancel(body: &[u8]) -> CliResponse {
    let parsed: CompressCancelBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match get_compress_job(&parsed.job_id) {
        Some(job) => {
            job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        None => CliResponse::bad_request(format!("unknown job_id: {}", parsed.job_id)),
    }
}

// ── zip-list (central directory, no extract) ──────────────────────────
//
// GET `fs/zip-list?path=` — names + sizes for the FileViewer zip preview.
// Read-only; zip-slip member names are still returned (extract rejects
// them). Cap + truncated flag live in `k2_core::fs_extract::list_zip_entries`.

/// GET `/cli/fs/zip-list?path=` — list central-directory entries.
pub fn handle_zip_list(params: &HashMap<String, String>) -> CliResponse {
    let path = match params.get("path") {
        Some(p) if !p.is_empty() => p.clone(),
        _ => return CliResponse::bad_request("Missing 'path' parameter"),
    };
    match k2_core::fs_extract::list_zip_entries(&path) {
        Ok(result) => CliResponse::ok_json(
            serde_json::to_string(&result)
                .unwrap_or_else(|_| r#"{"entries":[],"truncated":false}"#.to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

// ── extract (server-side zip → folder) ────────────────────────────────
//
// Same start-then-poll job shape as compress: `POST fs/extract` returns
// a job_id, a worker thread streams members into a sibling folder,
// `GET fs/extract-status?job_id=` snapshots progress, `POST
// fs/extract-cancel` raises the cooperative cancel flag.

/// An extract job's observable state; `status` snapshots it.
#[derive(Clone, serde::Serialize)]
pub struct ExtractJob {
    pub job_id: String,
    /// `running` → `done` | `failed`. Terminal states stay in the map so a
    /// poll that races completion still sees the outcome (jobs are removed
    /// only when a new job starts — see `insert_extract_job`).
    pub phase: &'static str,
    /// Entries written / entries in the archive.
    pub done: u64,
    pub total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dest_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip)]
    cancel: Arc<AtomicBool>,
}

fn extract_jobs() -> &'static Mutex<HashMap<String, ExtractJob>> {
    static JOBS: OnceLock<Mutex<HashMap<String, ExtractJob>>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn insert_extract_job(job: ExtractJob) {
    let mut map = extract_jobs().lock().unwrap_or_else(|e| e.into_inner());
    map.retain(|_, j| j.phase == "running");
    map.insert(job.job_id.clone(), job);
}

fn update_extract_job<F: FnOnce(&mut ExtractJob)>(job_id: &str, f: F) {
    if let Some(job) = extract_jobs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(job_id)
    {
        f(job);
    }
}

fn get_extract_job(job_id: &str) -> Option<ExtractJob> {
    extract_jobs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(job_id)
        .cloned()
}

fn new_extract_job_id() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::getrandom(&mut bytes).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        return format!("unzip-{nanos:x}");
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Deserialize)]
struct ExtractBody {
    path: String,
}

/// Start an extract job for the zip at `path`. Instant-fail validation
/// (missing path / not a .zip file) happens HERE so the caller gets a
/// 400 instead of a job that flips to `failed`; the deep work streams
/// on a worker thread. Returns `{ job_id }`.
pub fn handle_extract(body: &[u8]) -> CliResponse {
    let parsed: ExtractBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let path = match k2_core::fs_commands::validate_path(&parsed.path) {
        Ok(p) => p,
        Err(e) => return CliResponse::bad_request(e),
    };
    if !path.is_file() {
        return CliResponse::bad_request(format!("Not a regular file: {}", parsed.path));
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if !k2_core::fs_extract::is_zip_filename(&name) {
        return CliResponse::bad_request(format!("Not a .zip archive: {name}"));
    }

    let job_id = new_extract_job_id();
    let cancel = Arc::new(AtomicBool::new(false));
    insert_extract_job(ExtractJob {
        job_id: job_id.clone(),
        phase: "running",
        done: 0,
        total: 0,
        dest_path: None,
        error: None,
        cancel: cancel.clone(),
    });

    let src = parsed.path;
    let worker_job_id = job_id.clone();
    std::thread::spawn(move || {
        let progress_id = worker_job_id.clone();
        let result = k2_core::fs_extract::extract_from_zip(
            &src,
            &move |done, total| {
                update_extract_job(&progress_id, |j| {
                    j.done = done;
                    j.total = total;
                });
            },
            &cancel,
        );
        match result {
            Ok(path) => update_extract_job(&worker_job_id, |j| {
                j.phase = "done";
                j.dest_path = Some(path.to_string_lossy().to_string());
            }),
            Err(e) => {
                k2_core::log_debug!("[daemon] fs/extract — job {worker_job_id} FAILED: {e}");
                update_extract_job(&worker_job_id, |j| {
                    j.phase = "failed";
                    j.error = Some(e);
                });
            }
        }
    });

    CliResponse::ok_json(serde_json::json!({ "job_id": job_id }).to_string())
}

/// Snapshot an extract job (GET, `?job_id=`).
pub fn handle_extract_status(params: &HashMap<String, String>) -> CliResponse {
    let job_id = match params.get("job_id") {
        Some(id) if !id.is_empty() => id,
        _ => return CliResponse::bad_request("Missing 'job_id' parameter"),
    };
    match get_extract_job(job_id) {
        Some(job) => {
            CliResponse::ok_json(serde_json::to_string(&job).unwrap_or_else(|_| "{}".to_string()))
        }
        None => CliResponse::bad_request(format!("unknown job_id: {job_id}")),
    }
}

#[derive(Deserialize)]
struct ExtractCancelBody {
    job_id: String,
}

/// Raise a running extract job's cooperative cancel flag. The worker
/// notices between entries, removes its partial dest folder, and flips
/// the job to `failed` ("Extraction cancelled").
pub fn handle_extract_cancel(body: &[u8]) -> CliResponse {
    let parsed: ExtractCancelBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match get_extract_job(&parsed.job_id) {
        Some(job) => {
            job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        None => CliResponse::bad_request(format!("unknown job_id: {}", parsed.job_id)),
    }
}

#[derive(Deserialize)]
struct DuplicateBody {
    path: String,
}

pub fn handle_duplicate(body: &[u8]) -> CliResponse {
    let parsed: DuplicateBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match fsc::duplicate(&parsed.path) {
        Ok(new_path) => {
            crate::session_events::emit_fs_changed_for_paths([new_path.clone()]);
            CliResponse::ok_json(serde_json::json!({ "path": new_path }).to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct OpenBody {
    /// Local file path (open-finder) or URL (open-external).
    target: String,
}

pub fn handle_open_finder(body: &[u8]) -> CliResponse {
    let parsed: OpenBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match fsc::open_in_finder(&parsed.target) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_open_external(body: &[u8]) -> CliResponse {
    let parsed: OpenBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };

    // 0.40.34 — viewer-aware URL opening (closes the "Phase 3.1" caveat
    // in `fs_commands::open_external` for http/https targets).
    //
    // RULE IMPLEMENTED: for an http(s) target we ALWAYS broadcast the
    // app-level `open_url` session event (source "terminal-link") so
    // every connected app — local or across the K2 Connect tunnel — can
    // open it in a browser tab. We then SKIP the daemon-local
    // open/xdg-open shell-out whenever there is AT LEAST ONE live
    // `/cli/sessions/events` subscriber (`receiver_count() > 0` on the
    // broadcast bus — cheap, exact): the viewer's browser is the right
    // one, and on a headless server the local shell-out does nothing
    // anyway. Only with ZERO subscribers (nobody watching to open it)
    // do we fall back to the historical local open. Non-http targets
    // (Finder/file-manager paths) keep the current local behavior
    // unconditionally.
    if crate::browser_routes::validate_open_url(&parsed.target).is_ok() {
        let url = parsed.target.trim().to_string();
        let _ = crate::session_events::emit(crate::session_events::SessionEvent::OpenUrl {
            url,
            source: "terminal-link".to_string(),
        });
        let subscribers = crate::session_events::sender().receiver_count();
        if subscribers > 0 {
            return CliResponse::ok_json(
                serde_json::json!({
                    "success": true,
                    "message": format!(
                        "Routed to {subscribers} connected app subscriber(s)"
                    ),
                })
                .to_string(),
            );
        }
        // else: fall through to the local opener below.
    }

    match fsc::open_external(&parsed.target) {
        Ok(msg) => {
            CliResponse::ok_json(serde_json::json!({ "success": true, "message": msg }).to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[cfg(test)]
#[path = "fs_routes_tests.rs"]
mod tests;
