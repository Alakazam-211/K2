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
//! - `POST /cli/sandbox/open {project_path, preset_id}` — owner token only.
//!   Starts the clicked enabled preset in the workspace microVM. A body
//!   `command` or `args` is ignored. Does not require `K2_SANDBOX_API`.

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

const CANNOT_SANDBOX_BODY: &str =
    r#"{"error":"this daemon cannot sandbox (microVM backend unavailable)"}"#;

struct OpenPlan {
    workspace_path: String,
    /// Registered project handle (`resolve_workspace_slug`), not the folder basename.
    workspace_slug: String,
    command: String,
    args: Vec<String>,
}

/// Host plan for `POST /cli/sandbox/open`. Refuses a path that is not
/// `projects.path` and a preset that is missing or disabled. Does not call
/// `resolve_agent_command` (that falls through to Claude). Does not provision
/// a sandbox directory.
fn plan_sandbox_open(body: &[u8]) -> Result<OpenPlan, CliResponse> {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return Err(CliResponse::bad_request("invalid JSON body")),
    };
    let Some(project_path) = v
        .get("project_path")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
    else {
        return Err(CliResponse::bad_request("Missing project_path"));
    };
    let Some(preset_id) = v
        .get("preset_id")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
    else {
        return Err(CliResponse::bad_request("Missing preset_id"));
    };

    let (workspace_path, handle) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let path: String = match conn.query_row(
            "SELECT path FROM projects WHERE path = ?1",
            rusqlite::params![project_path],
            |r| r.get(0),
        ) {
            Ok(p) => p,
            Err(_) => {
                return Err(CliResponse::bad_request(
                    "project_path is not a registered project",
                ))
            }
        };
        let Some(handle) = k2_core::workspace::handle::project_handle_for_path(&conn, &path) else {
            return Err(CliResponse::bad_request(
                "project_path is not a registered project",
            ));
        };
        (path, handle)
    };
    // The spawn slug is the registered handle, not `slug_from_project_path`.
    match crate::v1_sandboxes::resolve_workspace_slug(&handle) {
        Some(resolved) if resolved == workspace_path => {}
        _ => {
            return Err(CliResponse::bad_request(
                "project_path is not a registered project",
            ))
        }
    }

    let preset_command = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match conn.query_row(
            "SELECT command FROM agent_presets WHERE id = ?1 AND enabled = 1",
            rusqlite::params![preset_id],
            |r| r.get::<_, String>(0),
        ) {
            Ok(cmd) => cmd,
            Err(_) => return Err(CliResponse::bad_request("preset is missing or disabled")),
        }
    };
    let (command, args) = k2_core::workspace::agent_resolve::parse_command_string(&preset_command);
    if command.is_empty() {
        return Err(CliResponse::bad_request("preset is missing or disabled"));
    }
    // Body `command` / `args` are not read. Model splice uses the workspace
    // default only — this door does not grow a caller command.
    let (command, args) =
        crate::v1_sandboxes::policy::finalize_jail_argv(&command, args, &workspace_path, None);
    Ok(OpenPlan {
        workspace_path,
        workspace_slug: handle,
        command,
        args,
    })
}

/// `POST /cli/sandbox/open`. Owner-gated at the dispatcher (`token_is_owner`).
/// `V1Principal::Owner` is the spawn identity; this function never sees the
/// daemon token. `can_sandbox() == false` returns 409 before any directory
/// or `sandbox_sessions` row is created, and does not fall through to a PTY.
pub fn handle_sandbox_open(body: &[u8]) -> CliResponse {
    let plan = match plan_sandbox_open(body) {
        Ok(plan) => plan,
        Err(resp) => return resp,
    };
    if !crate::v2_spawn::can_sandbox() {
        return CliResponse {
            status: "409 Conflict",
            content_type: "application/json",
            body: CANNOT_SANDBOX_BODY.to_string(),
        };
    }
    crate::v1_sandboxes::spawn_workspace_preset(
        plan.workspace_path,
        plan.workspace_slug,
        plan.command,
        plan.args,
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init() {
        k2_core::db::init_for_tests();
        assert!(
            !crate::v2_spawn::can_sandbox(),
            "test premise: this build cannot sandbox"
        );
    }

    fn claude_preset_id() -> String {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT id FROM agent_presets WHERE label = 'Claude' AND enabled = 1 LIMIT 1",
            [],
            |r| r.get(0),
        )
        .expect("seeded Claude preset")
    }

    fn insert_project(path: &str, handle: &str) {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle, default_agent) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                id,
                format!("Pretty {handle}"),
                path,
                handle,
                claude_preset_id(),
            ],
        )
        .expect("insert project");
    }

    fn insert_preset(id: &str, command: &str, enabled: i64) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO agent_presets \
                (id, label, command, icon, enabled, sort_order, is_built_in) \
             VALUES (?1, ?2, ?3, '', ?4, 999, 0)",
            rusqlite::params![id, format!("row-{id}"), command, enabled],
        )
        .expect("insert preset");
    }

    fn rows_for(slug: &str) -> i64 {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM sandbox_sessions WHERE workspace_slug = ?1",
            rusqlite::params![slug],
            |r| r.get(0),
        )
        .expect("count sandbox_sessions")
    }

    fn body(path: &str, preset: &str, extra: &str) -> Vec<u8> {
        format!(r#"{{"project_path":{path:?},"preset_id":{preset:?}{extra}}}"#).into_bytes()
    }

    #[test]
    fn missing_or_disabled_preset_refuses_without_claude_fallthrough() {
        init();
        let uniq = uuid::Uuid::new_v4();
        let path = format!("/tmp/k2-sbx-open-{uniq}");
        let handle = format!("hdl{uniq}").replace('-', "");
        insert_project(&path, &handle);
        let disabled = format!("off-{uniq}");
        insert_preset(&disabled, "codex --full-auto", 0);

        let missing = handle_sandbox_open(&body(&path, "no-such-preset", ""));
        assert_eq!(missing.status, "400 Bad Request", "{}", missing.body);
        assert!(missing.body.contains("preset is missing or disabled"));
        assert_ne!(missing.status, "409 Conflict");
        assert_eq!(rows_for(&handle), 0);

        let off = handle_sandbox_open(&body(&path, &disabled, ""));
        assert_eq!(off.status, "400 Bad Request", "{}", off.body);
        assert!(off.body.contains("preset is missing or disabled"));
        assert_ne!(off.status, "409 Conflict");
        assert_eq!(rows_for(&handle), 0);
    }

    #[test]
    fn worktree_path_refuses_and_slug_is_the_handle() {
        init();
        let uniq = uuid::Uuid::new_v4();
        let path = format!("/tmp/k2-sbx-open-{uniq}/foldername");
        let handle = format!("clicked{uniq}").replace('-', "");
        assert_ne!(handle, "foldername");
        insert_project(&path, &handle);
        let preset = format!("codex-{uniq}");
        insert_preset(&preset, "codex --full-auto", 1);

        let worktree = format!("{path}/worktrees/feat");
        let refused = handle_sandbox_open(&body(&worktree, &preset, ""));
        assert_eq!(refused.status, "400 Bad Request", "{}", refused.body);
        assert!(refused.body.contains("not a registered project"));
        assert_eq!(rows_for(&handle), 0);
        assert_eq!(rows_for("feat"), 0);

        let plan = plan_sandbox_open(&body(&path, &preset, ""))
            .unwrap_or_else(|e| panic!("plan: {} {}", e.status, e.body));
        assert_eq!(plan.workspace_slug, handle);
        assert_ne!(plan.workspace_slug, "foldername");
        assert_eq!(plan.command, "codex");
        assert_eq!(plan.args, vec!["--full-auto".to_string()]);
    }

    #[test]
    fn body_command_is_ignored_and_macos_409_creates_nothing() {
        init();
        let uniq = uuid::Uuid::new_v4();
        let path = format!("/tmp/k2-sbx-open-{uniq}");
        let handle = format!("cmd{uniq}").replace('-', "");
        insert_project(&path, &handle);
        let preset = format!("codex-{uniq}");
        insert_preset(&preset, "codex --full-auto", 1);
        let raw = body(
            &path,
            &preset,
            r#","command":"claude","args":["--dangerously-skip-permissions","evil"]"#,
        );
        let plan =
            plan_sandbox_open(&raw).unwrap_or_else(|e| panic!("plan: {} {}", e.status, e.body));
        assert_eq!(plan.command, "codex");
        assert_eq!(plan.args, vec!["--full-auto".to_string()]);
        assert!(!plan.args.iter().any(|a| a.contains("evil")));

        let resp = handle_sandbox_open(&raw);
        assert_eq!(resp.status, "409 Conflict", "{}", resp.body);
        assert_eq!(resp.body, CANNOT_SANDBOX_BODY);
        assert_eq!(rows_for(&handle), 0);
        assert_eq!(rows_for("foldername"), 0);
    }

    #[test]
    fn claude_row_ensures_skip_permissions_once() {
        init();
        let uniq = uuid::Uuid::new_v4();
        let path = format!("/tmp/k2-sbx-open-{uniq}");
        let handle = format!("cl{uniq}").replace('-', "");
        insert_project(&path, &handle);
        let preset = format!("claude-{uniq}");
        insert_preset(&preset, "claude", 1);
        let plan = plan_sandbox_open(&body(&path, &preset, ""))
            .unwrap_or_else(|e| panic!("plan: {} {}", e.status, e.body));
        assert_eq!(plan.command, "claude");
        assert_eq!(
            plan.args,
            vec!["--dangerously-skip-permissions".to_string()]
        );
    }

    #[test]
    fn open_arm_is_owner_gated_ahead_of_the_sandbox_prefix() {
        let src = include_str!("routes/dispatcher.rs");
        let open = src
            .find("p == \"/cli/sandbox/open\"")
            .expect("exact open arm");
        let prefix = src
            .find("p.starts_with(\"/cli/sandbox/\")")
            .expect("sandbox prefix");
        assert!(
            open < prefix,
            "open arm must precede the /cli/sandbox/ prefix"
        );
        let window = &src[open..prefix];
        assert!(window.contains("token_is_owner"), "{window}");
        assert!(!window.contains("token_ok"), "{window}");
        assert!(!window.contains("K2_SANDBOX_API"), "{window}");
        assert!(window.contains("handle_sandbox_open(&body_bytes)"));
        assert!(
            !window.contains("handle_sandbox_open(&body_bytes,"),
            "the handler must not receive the daemon token"
        );
        assert!(crate::routes::route_policy::post_allowed("/cli/sandbox/open"));
    }

    #[test]
    fn public_sandbox_request_grows_no_preset_or_command_field() {
        let src = include_str!("v1_sandboxes/policy.rs");
        let start = src
            .find("pub struct ApiSandboxRequest")
            .expect("ApiSandboxRequest");
        let end = src[start..]
            .find("\n}\n")
            .expect("end of ApiSandboxRequest");
        let block = &src[start..start + end];
        assert!(!block.contains("preset"), "{block}");
        assert!(!block.contains("command"), "{block}");
        assert!(!block.contains("args"), "{block}");
    }
}
