//! Sandbox-chat audit routes (owner cockpit) — the right-hand CHATS panel's
//! "Sandboxed" section. Lists the API-triggered sandbox sessions that ran in a
//! workspace and re-launches one INSIDE its sandbox (the audit-resume flow,
//! fs-mirror PRD §4/§5).
//!
//! - `GET  /cli/sandbox/list?project_path=<path>` — enumerate a workspace's
//!   sandbox sessions. Each session dir carries a daemon-owned `meta.json`
//!   (written at spawn by [`write_meta`]) so the daemon can list titles/times
//!   WITHOUT reading the cell-uid-owned `0700` `.claude` transcript (which it
//!   can't — no CAP_DAC_READ_SEARCH). Sessions predating meta.json still list
//!   with a generic title + the dir mtime.
//! - `POST /cli/sandbox/reopen {project_path, session_id[, prompt]}` — re-launch
//!   the session in its sandbox. Owner-authed; reuses the V1 address handler
//!   (`V1Principal::Owner` → live-deliver-or-resume, with K2_RESUME).

use crate::cli_response::CliResponse;
use crate::routes::http::V1Principal;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
struct SandboxChat {
    #[serde(rename = "sessionId")]
    session_id: String,
    title: String,
    timestamp: u64, // unix ms
    #[serde(rename = "messageCount")]
    message_count: u64,
}

/// Root of the per-workspace sandbox homes (daemon-home only — never a caller
/// path). Mirrors `v1_sandboxes::policy::sandbox_homes_root`.
pub fn sandbox_homes_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
        .join("sandbox-homes")
}

/// Workspace slug the sandbox homes are keyed by, from a project path (last
/// component, e.g. `/home/k2/ai` → `ai`). Rejects empties + traversal.
fn slug_from_project_path(p: &str) -> Option<String> {
    let name = Path::new(p.trim_end_matches('/'))
        .file_name()?
        .to_string_lossy()
        .into_owned();
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return None;
    }
    Some(name)
}

/// Write the daemon-owned per-session `meta.json` at spawn — the audit index the
/// list reads (the cell-owned `.claude` is unreadable to the daemon). Best-effort:
/// never fails a spawn. `ws_slug`/`sid` are already re-asserted by the caller.
pub fn write_meta(ws_slug: &str, sid: &str, title: &str, created_ms: u64) {
    let dir = sandbox_homes_root().join(ws_slug).join(sid);
    // First-spawn wins: a resume (same id) must NOT overwrite the original title.
    if dir.join("meta.json").exists() {
        return;
    }
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let meta = serde_json::json!({
        "title": title.chars().take(120).collect::<String>(),
        "created_ms": created_ms,
    });
    let _ = std::fs::write(dir.join("meta.json"), meta.to_string());
}

fn dir_mtime_ms(p: &Path) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `GET /cli/sandbox/list?project_path=<path>` — the workspace's sandbox sessions.
pub fn handle_sandbox_list(params: &HashMap<String, String>) -> CliResponse {
    let Some(project_path) = params.get("project_path").or_else(|| params.get("project")) else {
        return CliResponse::bad_request("Missing project_path parameter");
    };
    let Some(slug) = slug_from_project_path(project_path) else {
        return CliResponse::ok_json("[]".to_string());
    };
    let ws_root = sandbox_homes_root().join(&slug);
    let mut out: Vec<SandboxChat> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&ws_root) {
        for entry in rd.flatten() {
            let sid = entry.file_name().to_string_lossy().into_owned();
            let dir = entry.path();
            // A real sandbox session has a `.claude` (we can stat its existence
            // even though we can't read INTO the cell-owned 0700 dir).
            if !dir.join(".claude").exists() {
                continue;
            }
            // Prefer the daemon-owned meta.json; fall back to dir mtime + a
            // generic title for sessions that predate it.
            let (title, ts) = match std::fs::read_to_string(dir.join("meta.json"))
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            {
                Some(m) => (
                    m.get("title")
                        .and_then(|t| t.as_str())
                        .filter(|t| !t.is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("Sandbox session {}", &sid[..sid.len().min(8)])),
                    m.get("created_ms").and_then(|t| t.as_u64()).unwrap_or_else(|| dir_mtime_ms(&dir)),
                ),
                None => (
                    format!("Sandbox session {}", &sid[..sid.len().min(8)]),
                    dir_mtime_ms(&dir),
                ),
            };
            out.push(SandboxChat {
                session_id: sid,
                title,
                timestamp: ts,
                message_count: 0,
            });
        }
    }
    out.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    CliResponse::ok_json(serde_json::to_string(&out).unwrap_or_else(|_| "[]".to_string()))
}

/// `POST /cli/sandbox/reopen {project_path, session_id[, prompt]}` — re-launch a
/// sandbox session in its sandbox. Owner-authed; reuses the V1 address handler
/// (live-deliver-or-resume). The re-spawned cell surfaces as its orange tab.
pub fn handle_sandbox_reopen(body: &[u8]) -> CliResponse {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return CliResponse::bad_request("invalid JSON body"),
    };
    let Some(project_path) = v.get("project_path").and_then(|x| x.as_str()) else {
        return CliResponse::bad_request("Missing project_path");
    };
    let Some(session_id) = v.get("session_id").and_then(|x| x.as_str()) else {
        return CliResponse::bad_request("Missing session_id");
    };
    let Some(slug) = slug_from_project_path(project_path) else {
        return CliResponse::bad_request("bad project_path");
    };
    crate::v1_sandboxes::handle_v1_ws_address(&V1Principal::Owner, &slug, session_id, body)
}
