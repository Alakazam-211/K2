//! Phase 2 Unit 4 — daemon-side `/cli/git/*` route handlers.
//!
//! libgit2 operations on large repos (status, diff, log) can take 100s
//! of ms. Per F5, these handlers run inside `tokio::task::spawn_blocking`
//! so they don't stall the accept loop. The dispatch in `main.rs`
//! wraps `dispatch_unit4_git_post` accordingly; the GET branches in
//! `cli::dispatch` do the same wrap inline.
//!
//! Worktree create/remove additionally touch the `workspaces` SQLite
//! table — those operations were Tauri-owned pre-Unit-4 and now live
//! here so the daemon is the sole DB writer.

use std::collections::HashMap;

use serde::Deserialize;

use crate::cli_response::CliResponse;
use k2_core::db;
use k2_core::db::schema::Workspace;
use k2_core::git;

// ── Helpers ───────────────────────────────────────────────────────────

fn ok_serialized<T: serde::Serialize>(value: T) -> CliResponse {
    match serde_json::to_string(&value) {
        Ok(s) => CliResponse::ok_json(s),
        Err(e) => CliResponse::internal_error(format!("serialize: {e}")),
    }
}

fn serialized<T: serde::Serialize>(r: Result<T, String>) -> CliResponse {
    match r {
        Ok(v) => ok_serialized(v),
        Err(e) => CliResponse::bad_request(e),
    }
}

fn unit_ok(r: Result<(), String>) -> CliResponse {
    match r {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

fn parse_body<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, CliResponse> {
    serde_json::from_slice(body).map_err(|e| CliResponse::bad_request(format!("invalid JSON: {e}")))
}

fn str_param(params: &HashMap<String, String>, key: &str) -> String {
    params.get(key).cloned().unwrap_or_default()
}

// ══════════════════════════════════════════════════════════════════════
// GET handlers
// ══════════════════════════════════════════════════════════════════════
//
// These are blocking (libgit2) — main.rs::cli::dispatch wraps the
// call site in spawn_blocking; the handler itself is sync.

pub fn handle_git_info(params: &HashMap<String, String>) -> CliResponse {
    let path = str_param(params, "path");
    if path.is_empty() {
        return CliResponse::bad_request("Missing 'path' parameter");
    }
    ok_serialized(git::get_git_info(&path))
}

pub fn handle_git_branches(params: &HashMap<String, String>) -> CliResponse {
    let path = str_param(params, "path");
    if path.is_empty() {
        return CliResponse::bad_request("Missing 'path' parameter");
    }
    ok_serialized(git::list_branches(&path))
}

pub fn handle_git_worktrees(params: &HashMap<String, String>) -> CliResponse {
    let path = str_param(params, "path");
    if path.is_empty() {
        return CliResponse::bad_request("Missing 'path' parameter");
    }
    ok_serialized(git::list_worktrees(&path))
}

pub fn handle_git_changes(params: &HashMap<String, String>) -> CliResponse {
    let path = str_param(params, "path");
    if path.is_empty() {
        return CliResponse::bad_request("Missing 'path' parameter");
    }
    ok_serialized(git::get_changed_files(&path))
}

pub fn handle_git_diff_file(params: &HashMap<String, String>) -> CliResponse {
    let path = str_param(params, "path");
    let file_path = str_param(params, "file_path");
    if path.is_empty() || file_path.is_empty() {
        return CliResponse::bad_request("Missing 'path'/'file_path' parameter");
    }
    serialized(git::diff_file(&path, &file_path))
}

pub fn handle_git_diff_summary(params: &HashMap<String, String>) -> CliResponse {
    let path = str_param(params, "path");
    if path.is_empty() {
        return CliResponse::bad_request("Missing 'path' parameter");
    }
    serialized(git::diff_summary(&path))
}

pub fn handle_git_diff_between_branches(params: &HashMap<String, String>) -> CliResponse {
    let path = str_param(params, "path");
    let base = str_param(params, "base_branch");
    let head = str_param(params, "head_branch");
    if path.is_empty() || base.is_empty() || head.is_empty() {
        return CliResponse::bad_request(
            "Missing 'path'/'base_branch'/'head_branch' parameter",
        );
    }
    serialized(git::diff_between_branches(&path, &base, &head))
}

pub fn handle_git_file_at_ref(params: &HashMap<String, String>) -> CliResponse {
    let path = str_param(params, "path");
    let file_path = str_param(params, "file_path");
    let git_ref = str_param(params, "git_ref");
    if path.is_empty() || file_path.is_empty() || git_ref.is_empty() {
        return CliResponse::bad_request("Missing 'path'/'file_path'/'git_ref' parameter");
    }
    serialized(git::file_content_at_ref(&path, &file_path, &git_ref))
}

pub fn handle_git_merge_status(params: &HashMap<String, String>) -> CliResponse {
    let path = str_param(params, "path");
    if path.is_empty() {
        return CliResponse::bad_request("Missing 'path' parameter");
    }
    serialized(git::merge_status(&path))
}

// ══════════════════════════════════════════════════════════════════════
// POST handlers
// ══════════════════════════════════════════════════════════════════════

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateWorktreeBody {
    project_path: String,
    branch: String,
    project_id: String,
    existing_branch: Option<bool>,
}

pub fn handle_git_create_worktree(body: &[u8]) -> CliResponse {
    let b: CreateWorktreeBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };

    let result = if b.existing_branch.unwrap_or(false) {
        git::checkout_worktree(&b.project_path, &b.branch)
    } else {
        git::create_worktree(&b.project_path, &b.branch)
    };
    let result = match result {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(e),
    };

    let db = db::shared();
    let conn = db.lock();
    let ws_id = uuid::Uuid::new_v4().to_string();
    let existing = Workspace::list(&conn, &b.project_id).unwrap_or_default();
    let max_order = existing.iter().map(|w| w.tab_order).max().unwrap_or(-1) + 1;
    if let Err(e) = Workspace::create(
        &conn,
        &ws_id,
        &b.project_id,
        None,
        "worktree",
        Some(&result.branch),
        &result.branch,
        max_order,
        Some(&result.path),
    ) {
        return CliResponse::bad_request(e.to_string());
    }

    ok_serialized(serde_json::json!({
        "workspaceId": ws_id,
        "path": result.path,
        "branch": result.branch,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoveWorktreeBody {
    project_path: String,
    worktree_path: String,
    workspace_id: Option<String>,
    force: Option<bool>,
}

pub fn handle_git_remove_worktree(body: &[u8]) -> CliResponse {
    let b: RemoveWorktreeBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(e) = git::remove_worktree(&b.project_path, &b.worktree_path, b.force.unwrap_or(false))
    {
        return CliResponse::bad_request(e);
    }
    if let Some(ws_id) = b.workspace_id {
        let db = db::shared();
        let conn = db.lock();
        if let Err(e) = Workspace::delete(&conn, &ws_id) {
            return CliResponse::bad_request(e.to_string());
        }
    }
    CliResponse::ok_json(r#"{"success":true}"#.to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReopenWorktreeBody {
    project_path: String,
    worktree_path: String,
    branch: String,
}

pub fn handle_git_reopen_worktree(body: &[u8]) -> CliResponse {
    let b: ReopenWorktreeBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if !std::path::Path::new(&b.worktree_path).exists() {
        return CliResponse::bad_request(format!(
            "Worktree path does not exist: {}",
            b.worktree_path
        ));
    }
    ok_serialized(serde_json::json!({
        "path": b.worktree_path,
        "branch": b.branch,
        "projectPath": b.project_path,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StageBody {
    path: String,
    file_path: String,
}

pub fn handle_git_stage(body: &[u8]) -> CliResponse {
    let b: StageBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(git::stage_file(&b.path, &b.file_path))
}

pub fn handle_git_unstage(body: &[u8]) -> CliResponse {
    let b: StageBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(git::unstage_file(&b.path, &b.file_path))
}

#[derive(Deserialize)]
struct PathBody {
    path: String,
}

pub fn handle_git_stage_all(body: &[u8]) -> CliResponse {
    let b: PathBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(git::stage_all(&b.path))
}

#[derive(Deserialize)]
struct CommitBody {
    path: String,
    message: String,
}

pub fn handle_git_commit(body: &[u8]) -> CliResponse {
    let b: CommitBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    serialized(git::commit(&b.path, &b.message))
}

#[derive(Deserialize)]
struct MergeBranchBody {
    path: String,
    branch: String,
}

pub fn handle_git_merge_branch(body: &[u8]) -> CliResponse {
    let b: MergeBranchBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    serialized(git::merge_branch(&b.path, &b.branch))
}

pub fn handle_git_abort_merge(body: &[u8]) -> CliResponse {
    let b: PathBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(git::abort_merge(&b.path))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveBody {
    path: String,
    file_path: String,
    resolution: String,
}

pub fn handle_git_resolve_conflict(body: &[u8]) -> CliResponse {
    let b: ResolveBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(git::resolve_conflict(&b.path, &b.file_path, &b.resolution))
}

#[derive(Deserialize)]
struct DeleteBranchBody {
    path: String,
    branch: String,
}

pub fn handle_git_delete_branch(body: &[u8]) -> CliResponse {
    let b: DeleteBranchBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    unit_ok(git::delete_branch(&b.path, &b.branch))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PruneWorktreesBody {
    project_path: String,
    project_id: String,
}

pub fn handle_git_prune_worktrees(body: &[u8]) -> CliResponse {
    let b: PruneWorktreesBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let _ = std::process::Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(&b.project_path)
        .output();

    let db = db::shared();
    let conn = db.lock();
    let workspaces = Workspace::list(&conn, &b.project_id).unwrap_or_default();
    for ws in &workspaces {
        if ws.type_ == "worktree" {
            if let Some(ref wt_path) = ws.worktree_path {
                if !std::path::Path::new(wt_path).exists() {
                    let _ = Workspace::delete(&conn, &ws.id);
                }
            }
        }
    }
    CliResponse::ok_json(r#"{"success":true}"#.to_string())
}

// ══════════════════════════════════════════════════════════════════════
// POST dispatch
// ══════════════════════════════════════════════════════════════════════

pub fn dispatch_unit4_git_post(path: &str, body: &[u8]) -> CliResponse {
    match path {
        "/cli/git/create-worktree" => handle_git_create_worktree(body),
        "/cli/git/remove-worktree" => handle_git_remove_worktree(body),
        "/cli/git/reopen-worktree" => handle_git_reopen_worktree(body),
        "/cli/git/stage" => handle_git_stage(body),
        "/cli/git/unstage" => handle_git_unstage(body),
        "/cli/git/stage-all" => handle_git_stage_all(body),
        "/cli/git/commit" => handle_git_commit(body),
        "/cli/git/merge-branch" => handle_git_merge_branch(body),
        "/cli/git/abort-merge" => handle_git_abort_merge(body),
        "/cli/git/resolve" => handle_git_resolve_conflict(body),
        "/cli/git/delete-branch" => handle_git_delete_branch(body),
        "/cli/git/prune-worktrees" => handle_git_prune_worktrees(body),
        _ => CliResponse::not_found(),
    }
}
