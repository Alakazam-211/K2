//! Downloading exposed files (prd-app-caps-0464-v1 §4, AC15–AC23, 0.46.4).
//!
//! ```text
//! GET /cli/fs/downloads ?workspace=<room>                              files.downloads
//!   → {ok, items: [{relPath, name, size, modifiedAt, mime}], truncated}
//! GET /cli/fs/download  ?workspace=<room>&path=<rel>[&offset=&len=]    files.download
//!   → {ok, base64, len, offset, size, eof, name, mime}
//! ```
//!
//! An App guest's pass only (the browser helper or the App's link, App
//! rows with no Garden door: a Garden is the owner and already reads every
//! file, AC23). Both need `files:download` in the room (person-only to
//! grant, unchanged). A file may be saved when **both** hold (AC17):
//!
//! 1. `files:download` in the room; and
//! 2. the path, as written **and** where it lands (symlinks followed), is
//!    inside one of the role line's download entries (never `.k2/`, never
//!    a secret-named file), **or** the login holds `files:read` there and
//!    the path passes today's `PathNeed::Read` scope (the bytes it could
//!    already read through `files.readBinary`, now save-shaped).
//!
//! Anything else is the plain missing-cap 403 naming `files:download`,
//! decided before the disk is touched (no existence oracle). An empty
//! download list exposes nothing, so a `files:download` holder without
//! entries and without `files:read` gets nothing new (AC37). Each saved
//! chunk leaves one audit line (`files.download`). Adding or widening
//! entries is person-only (`skin_routes::handle_roles_paths`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use k2_core::fs_commands as fsc;
use k2_core::skin::paths::{self as sp, PathNeed};
use k2_core::skin::{RoomPathScope, SkinPass, CAP_FILES_DOWNLOAD};

use crate::cli_response::CliResponse;
use crate::fs_routes::{
    abs_in_agent_config, jail_rel_path, lexically_in_agent_config, need_rel_path, need_workspace, rel_in_room,
    resolve_skin_workspace, room_scope, scope_gate_landed, SkinWorkspace,
};

pub const ROUTE_LIST: &str = "/cli/fs/downloads";
pub const ROUTE_GET: &str = "/cli/fs/download";

/// `files.downloads`: a folder entry walks at most this many levels…
pub const MAX_DEPTH: usize = 8;
/// …and the answer holds at most this many files in all.
pub const MAX_ITEMS: usize = 500;
/// `files.download` chunk sizes (the same as `files.readRange`).
pub const LEN_DEFAULT: u64 = 8 * 1024 * 1024;
pub const LEN_MAX: u64 = 16 * 1024 * 1024;


fn refused() -> CliResponse {
    crate::skin_routes::missing_cap_response(CAP_FILES_DOWNLOAD)
}

/// The owner token and Connect logins have no download entries: these are
/// an App guest's doors (the owner reads files with `/cli/fs/read-range`).
pub fn not_a_guest() -> CliResponse {
    CliResponse {
        status: "400 Bad Request",
        content_type: "application/json",
        body: serde_json::json!({
            "error": "usage",
            "message": "files.download and files.downloads are an App guest's doors (its own pass); the owner reads files with /cli/fs/read-range",
        })
        .to_string(),
    }
}

fn who(pass: &SkinPass) -> String {
    format!("guest:{}", pass.username.trim())
}

/// The room, `files:download` in it and the line's scope; else the
/// refusal (the room's own `skin_room`, or the missing-cap 403; a scope
/// that can't be read fails closed with the same 403).
fn gate(params: &HashMap<String, String>, pass: &SkinPass) -> Result<(SkinWorkspace, RoomPathScope), CliResponse> {
    let ws = need_workspace(params)?;
    let resolved = resolve_skin_workspace(pass, &ws)?;
    if !pass.has_cap_in_room(&resolved.project_id, CAP_FILES_DOWNLOAD) {
        return Err(refused());
    }
    let scope = room_scope(pass, &resolved, PathNeed::Read).map_err(|_| refused())?;
    Ok((resolved, scope))
}

/// Paths the download door never serves, by name (review A3): K2's own
/// secret patterns, plus anything in a `.git/` folder and these files.
fn denied_by_name(rel: &str) -> bool {
    if k2_core::secret_paths::is_secret_rel(rel) {
        return true;
    }
    let parts: Vec<String> = rel.split(['/', '\\']).filter(|p| !p.is_empty() && *p != ".").map(str::to_ascii_lowercase).collect();
    let Some(name) = parts.last() else { return true };
    if parts.iter().any(|p| p == ".git") {
        return true;
    }
    if matches!(name.as_str(), ".env.k2" | ".pgpass" | "terraform.tfstate" | "terraform.tfstate.backup") || name.ends_with(".p8") {
        return true;
    }
    let tail2 = |a: &str, b: &str| parts.len() >= 2 && parts[parts.len() - 2] == a && parts[parts.len() - 1] == b;
    tail2(".aws", "credentials") || tail2(".kube", "config") || tail2(".docker", "config.json")
}

/// May a download entry serve the room-relative `rel` (as written)?
fn entry_allows_rel(scope: &RoomPathScope, rel: &str) -> bool {
    match sp::rel_segments(rel) {
        Ok(segs) => !segs.is_empty() && sp::download_allows(scope, &segs) && !lexically_in_agent_config(rel, false) && !denied_by_name(rel),
        Err(_) => false,
    }
}

/// The landed check for the entry arm: the resolved path is still inside a
/// download entry, never in `.k2/`, never denied by name.
fn entry_allows_landed(scope: &RoomPathScope, resolved: &SkinWorkspace, real: &Path) -> bool {
    let Some(landed) = rel_in_room(&resolved.path, real) else { return false };
    entry_allows_rel(scope, &landed) && !abs_in_agent_config(&resolved.path, real, false)
}

/// Review A3: every file listed or served also passes K2's secret
/// classifier on the landed path (names and the content scan).
///
/// Known limit (round 2, documented): the content scan reads text files up
/// to `secret_paths::MAX_SCAN_BYTES` (1 MiB). A larger file, or one with
/// NUL bytes (binary), is served on its name and path alone, so a secret
/// inside a big or binary file is not caught: expose files, not folders,
/// when that matters.
fn secret_file(resolved: &SkinWorkspace, real: &Path) -> bool {
    let Some(landed) = rel_in_room(&resolved.path, real) else { return true };
    denied_by_name(&landed) || k2_core::secret_paths::classify(&landed, real).is_some()
}

/// Round 2 A3: a hard link (`nlink > 1`) may be another path's file under
/// a harmless name; the download door never lists or serves one.
fn hard_linked(meta: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        meta.nlink() > 1
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        false
    }
}

/// Round 2 A3: the content bytes one listing may scan (each text file
/// costs up to 1 MiB); past it the listing stops and says `truncated`.
pub const LIST_SCAN_BUDGET: u64 = 64 * 1024 * 1024;

/// `GET /cli/fs/downloads` for an App guest.
pub fn handle_list(params: &HashMap<String, String>, pass: &SkinPass) -> CliResponse {
    let (resolved, scope) = match gate(params, pass) {
        Ok(g) => g,
        Err(r) => {
            crate::thread_attachments::audit("files.downloads_refused", &who(pass), "list".to_string());
            return r;
        }
    };
    let mut items: Vec<serde_json::Value> = Vec::new();
    let mut seen_files: HashSet<PathBuf> = HashSet::new();
    let mut truncated = false;
    let mut scan_left = LIST_SCAN_BUDGET;
    for entry in &scope.download {
        if truncated {
            break;
        }
        if !entry_allows_rel(&scope, entry) {
            continue;
        }
        // Missing or escaping entries are left out.
        let Ok(real) = jail_rel_path(&resolved.path, entry, false) else { continue };
        if !entry_allows_landed(&scope, &resolved, &real) {
            continue;
        }
        let mut visited: HashSet<PathBuf> = HashSet::new();
        truncated = walk(&scope, &resolved, &real, 0, &mut visited, &mut seen_files, &mut items, &mut scan_left);
    }
    crate::thread_attachments::audit(
        "files.downloads",
        &who(pass),
        format!("room={} items={} truncated={truncated}", resolved.project_id, items.len()),
    );
    CliResponse::ok_json(serde_json::json!({ "ok": true, "items": items, "truncated": truncated }).to_string())
}

/// Add `real` (a file, or a folder walked to [`MAX_DEPTH`]) to `items`.
/// Returns `true` once [`MAX_ITEMS`] is passed or the scan budget is spent
/// (the walk stops). `.git` folders are never walked; secret and
/// hard-linked files are never listed.
#[allow(clippy::too_many_arguments)]
fn walk(
    scope: &RoomPathScope,
    resolved: &SkinWorkspace,
    real: &Path,
    depth: usize,
    visited: &mut HashSet<PathBuf>,
    seen_files: &mut HashSet<PathBuf>,
    items: &mut Vec<serde_json::Value>,
    scan_left: &mut u64,
) -> bool {
    let Ok(meta) = std::fs::metadata(real) else { return false };
    if meta.is_file() {
        if items.len() >= MAX_ITEMS {
            return true;
        }
        if !seen_files.insert(real.to_path_buf()) || hard_linked(&meta) {
            return false;
        }
        // A file past the scan limit isn't read (it costs nothing).
        let cost = if meta.len() <= k2_core::secret_paths::MAX_SCAN_BYTES { meta.len() } else { 0 };
        if cost > *scan_left {
            return true;
        }
        *scan_left -= cost;
        if secret_file(resolved, real) {
            return false;
        }
        if let Some(row) = item(resolved, real, &meta) {
            items.push(row);
        }
        return false;
    }
    if !meta.is_dir() || depth >= MAX_DEPTH || !visited.insert(real.to_path_buf()) {
        return false;
    }
    let Ok(rd) = std::fs::read_dir(real) else { return false };
    let mut children: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    children.sort();
    for child in children {
        if child.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(".git")) {
            continue;
        }
        // Symlinks are followed only when they land inside a download
        // entry (and inside the room): the same landed rule as a save.
        let Ok(canon) = dunce::canonicalize(&child) else { continue };
        if !entry_allows_landed(scope, resolved, &canon) {
            continue;
        }
        if walk(scope, resolved, &canon, depth + 1, visited, seen_files, items, scan_left) {
            return true;
        }
    }
    false
}

fn item(resolved: &SkinWorkspace, real: &Path, meta: &std::fs::Metadata) -> Option<serde_json::Value> {
    let rel = rel_in_room(&resolved.path, real)?;
    let name = real.file_name()?.to_string_lossy().to_string();
    let modified_at = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    Some(serde_json::json!({
        "relPath": rel,
        "mime": fsc::mime_for_name(&name),
        "name": name,
        "size": meta.len(),
        "modifiedAt": modified_at,
    }))
}

/// `GET /cli/fs/download` for an App guest.
pub fn handle_get(params: &HashMap<String, String>, pass: &SkinPass) -> CliResponse {
    let (resolved, scope) = match gate(params, pass) {
        Ok(g) => g,
        Err(r) => {
            crate::thread_attachments::audit("files.download_refused", &who(pass), "gate".to_string());
            return r;
        }
    };
    let rel = match need_rel_path(params) {
        Ok(p) => p,
        Err(e) => return e,
    };
    // A malformed path is the jail's own 400, as on every files door.
    if let Err(e) = sp::rel_segments(&rel) {
        return CliResponse::bad_request(e);
    }
    let offset: u64 = match params.get("offset").map(|v| v.trim().parse()) {
        Some(Ok(v)) => v,
        Some(Err(_)) => return CliResponse::bad_request("invalid 'offset' parameter"),
        None => 0,
    };
    let len: u64 = match params.get("len").map(|v| v.trim().parse()) {
        Some(Ok(v)) => v,
        Some(Err(_)) => return CliResponse::bad_request("invalid 'len' parameter"),
        None => LEN_DEFAULT,
    };
    if len == 0 || len > LEN_MAX {
        return CliResponse::bad_request(format!("len is 1..={LEN_MAX}"));
    }
    let deny = |why: &str| {
        crate::thread_attachments::audit(
            "files.download_refused",
            &who(pass),
            format!("room={} rel={rel:?} why={why}", resolved.project_id),
        );
        refused()
    };
    // Rule 2, as written, before the disk is touched.
    let by_entry = entry_allows_rel(&scope, &rel);
    let by_read = pass.has_cap_in_room(&resolved.project_id, crate::skin_routes::FILES_READ)
        && sp::allows_rel(&scope, &rel, PathNeed::Read).unwrap_or(false);
    if !by_entry && !by_read {
        return deny("outside");
    }
    // Review A6: from here on, missing, a link out, a folder and a secret
    // are all the same missing-cap 403 (no oracle inside an entry).
    let Ok(real) = jail_rel_path(&resolved.path, &rel, false) else { return deny("unresolved") };
    let landed_entry = by_entry && entry_allows_landed(&scope, &resolved, &real);
    let landed_read = by_read && scope_gate_landed(&scope, &resolved, &real, PathNeed::Read, true).is_none();
    if !landed_entry && !landed_read {
        return deny("landed");
    }
    match std::fs::metadata(&real) {
        Ok(m) if m.is_file() && !hard_linked(&m) => {}
        Ok(m) if m.is_file() => return deny("hard_link"),
        _ => return deny("not_a_file"),
    }
    if secret_file(&resolved, &real) {
        return deny("secret");
    }
    let landed = rel_in_room(&resolved.path, &real).unwrap_or_else(|| rel.clone());
    let name = real.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    match fsc::read_file_range(&real.to_string_lossy(), offset, len) {
        Ok((bytes, size)) => {
            let eof = offset.saturating_add(bytes.len() as u64) >= size;
            // The path actually read (review A6).
            crate::thread_attachments::audit(
                "files.download",
                &who(pass),
                format!(
                    "room={} rel={landed:?} asked={rel:?} offset={offset} len={} via={}",
                    resolved.project_id,
                    bytes.len(),
                    if landed_entry { "entry" } else { "files:read" }
                ),
            );
            CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "base64": B64.encode(&bytes),
                    "len": bytes.len() as u64,
                    "offset": offset,
                    "size": size,
                    "eof": eof,
                    "mime": fsc::mime_for_name(&name),
                    "name": name,
                })
                .to_string(),
            )
        }
        Err(e) => {
            k2_core::log_debug!("[fs/download] read {landed:?}: {e}");
            deny("read")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(download: &[&str]) -> RoomPathScope {
        sp::normalize_scope(&RoomPathScope { download: download.iter().map(|s| s.to_string()).collect(), ..Default::default() })
            .expect("scope")
    }

    #[test]
    fn entries_never_serve_k2_secrets_or_malformed_paths() {
        let s = scope(&["public", "reports/a.pdf"]);
        assert!(entry_allows_rel(&s, "public/x.txt"));
        assert!(entry_allows_rel(&s, "reports/a.pdf"));
        assert!(!entry_allows_rel(&s, "reports/b.pdf"));
        assert!(!entry_allows_rel(&s, "public/../x"));
        assert!(!entry_allows_rel(&s, "."), "the room root is never an entry");
        assert!(!entry_allows_rel(&scope(&[]), "anything"), "no entries: nothing");
        // Review A3: names the door never serves.
        for rel in [
            "public/.env",
            "public/id_rsa",
            "public/.env.k2",
            "public/.git/config",
            "public/repo/.git/HEAD",
            "public/.GIT/objects/ab",
            "public/.aws/credentials",
            "public/.kube/config",
            "public/.docker/config.json",
            "public/.pgpass",
            "public/AuthKey_ABC123.p8",
            "public/infra/terraform.tfstate",
            "public/infra/terraform.tfstate.backup",
        ] {
            assert!(!entry_allows_rel(&s, rel), "{rel} must never be served");
        }
        // Their near-misses are ordinary files.
        for rel in ["public/kube-config.md", "public/docker/config.json.example", "public/notes.p8x", "public/git/HEAD"] {
            assert!(entry_allows_rel(&s, rel), "{rel} is an ordinary file");
        }
    }
}
