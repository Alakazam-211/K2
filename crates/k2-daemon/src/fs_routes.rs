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

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::Deserialize;

use crate::cli_response::CliResponse;
use k2_core::fs_commands as fsc;

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
    match fsc::read_dir(&path, show_hidden) {
        Ok(entries) => CliResponse::ok_json(
            serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_read_file(params: &HashMap<String, String>) -> CliResponse {
    let path = match params.get("path") {
        Some(p) if !p.is_empty() => p.clone(),
        _ => return CliResponse::bad_request("Missing 'path' parameter"),
    };
    match fsc::read_file(&path) {
        Ok(content) => CliResponse::ok_json(
            serde_json::to_string(&content).unwrap_or_else(|_| "{}".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
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

/// Report the daemon machine's filesystem basics so a remote client can
/// seed a folder browser at the host's home dir (instead of a hardcoded
/// `/`) and render paths with the host's separator. No path input — purely
/// describes the host. Gated like the other `fs/*` reads (`token_ok`).
pub fn handle_info(_params: &HashMap<String, String>) -> CliResponse {
    let home = dirs::home_dir()
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
        Ok(paths) => CliResponse::ok_json(
            serde_json::to_string(&paths).unwrap_or_else(|_| "[]".to_string()),
        ),
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
struct WriteFileBody {
    path: String,
    content: String,
}

pub fn handle_write_file(body: &[u8]) -> CliResponse {
    let parsed: WriteFileBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match fsc::write_file(&parsed.path, &parsed.content) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct MoveCopyBody {
    sources: Vec<String>,
    destination: String,
}

pub fn handle_move(body: &[u8]) -> CliResponse {
    let parsed: MoveCopyBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match fsc::move_files(&parsed.sources, &parsed.destination) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_copy(body: &[u8]) -> CliResponse {
    let parsed: MoveCopyBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match fsc::copy_files(&parsed.sources, &parsed.destination) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
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
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
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
    match fsc::rename(&parsed.old_path, &parsed.new_name) {
        Ok(new_path) => CliResponse::ok_json(
            serde_json::json!({ "path": new_path }).to_string(),
        ),
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
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
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
        Ok(path) => CliResponse::ok_json(
            serde_json::json!({ "path": path.to_string_lossy() }).to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
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
        Ok(new_path) => CliResponse::ok_json(
            serde_json::json!({ "path": new_path }).to_string(),
        ),
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
    match fsc::open_external(&parsed.target) {
        Ok(msg) => CliResponse::ok_json(
            serde_json::json!({ "success": true, "message": msg }).to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_returns_home_separator_and_os() {
        let resp = handle_info(&HashMap::new());
        assert_eq!(resp.status, "200 OK", "fs/info must succeed");

        let v: serde_json::Value =
            serde_json::from_str(&resp.body).expect("fs/info body must be JSON");

        // separator is exactly the host's MAIN_SEPARATOR.
        assert_eq!(
            v["separator"].as_str().expect("separator must be a string"),
            std::path::MAIN_SEPARATOR.to_string(),
        );

        // os is the compile-target OS string.
        assert_eq!(
            v["os"].as_str().expect("os must be a string"),
            std::env::consts::OS,
        );

        // home is present as a string (may be empty if unavailable, but the
        // key must exist and be string-typed — not null/missing).
        assert!(
            v["home"].is_string(),
            "home must be a string, got {:?}",
            v["home"],
        );
    }
}
