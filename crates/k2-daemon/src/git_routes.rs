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
use k2_core::git_changes;

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

fn flag_param(params: &HashMap<String, String>, key: &str) -> bool {
    matches!(params.get(key).map(String::as_str), Some("1") | Some("true") | Some("on") | Some("yes"))
}

/// Changes tab errors → `{"error", "code"}` with the matching status.
fn changes_error(e: git_changes::ChangesError) -> CliResponse {
    let status = match e.code {
        git_changes::codes::NOT_FOUND => "404 Not Found",
        git_changes::codes::CONFLICTED => "409 Conflict",
        _ => "400 Bad Request",
    };
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({ "error": e.hint, "code": e.code }).to_string(),
    }
}

/// Refuse a stage/unstage file path that escapes the repo (C1 hardening).
/// Returns the repo-relative path to use.
fn checked_rel(path: &str, file_path: &str) -> Result<String, CliResponse> {
    let Some((_, root)) = git_changes::repo_key_and_root(path) else {
        // Not a repo: let the git op report it exactly as before.
        return Ok(file_path.to_string());
    };
    git_changes::normalize_rel_path(std::path::Path::new(&root), file_path).map_err(changes_error)
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
    let mut v = serde_json::to_value(git::get_git_info(&path)).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(obj) = v.as_object_mut() {
        // Changes tab (C5 scope add): the + menu offers Changes for every
        // workspace while this server can do git. Status, diff, stage,
        // commit, discard and `git/init` use the linked libgit2, so that is
        // always true here; `gitCli` says whether the `git` binary (used for
        // worktree create/remove/prune) is on PATH.
        obj.insert("gitAvailable".into(), serde_json::Value::Bool(true));
        obj.insert("gitCli".into(), serde_json::Value::Bool(git_cli_present()));
    }
    ok_serialized(v)
}

/// Is the `git` binary runnable on this server (checked once per daemon)?
pub fn git_cli_present() -> bool {
    static PRESENT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PRESENT.get_or_init(|| {
        std::process::Command::new("git")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

#[derive(Deserialize)]
struct InitBody {
    #[serde(default)]
    path: String,
}

/// Changes tab (C5 scope add) — `POST /cli/git/init` `{path}`: make the
/// workspace root a git repository (libgit2 `git init`: a `.git` folder,
/// no commit). Only a registered K2 workspace root (never a folder above
/// or below one) and only when no repository already holds it (409
/// `already_a_repo`). Then `git_changed` fires and the watcher picks up
/// the new `.git`, so an open Changes tab shows the repo at once.
pub fn handle_git_init(is_post: bool, body: &[u8]) -> CliResponse {
    if !is_post {
        return CliResponse::method_not_allowed();
    }
    let b: InitBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let t = b.path.trim();
    let ws = if t.len() > 1 { t.trim_end_matches('/') } else { t };
    if ws.is_empty() {
        return changes_error(git_changes::ChangesError::new(git_changes::codes::BAD_REQUEST, "Missing 'path'"));
    }
    let root = match crate::git_notes_routes::owning_project(ws) {
        Some((_, root)) if root == ws => root,
        Some((_, root)) => {
            return changes_error(git_changes::ChangesError::new(
                git_changes::codes::BAD_PATH,
                format!("only a workspace root can become a repository; {ws} is inside {root}"),
            ))
        }
        None => {
            return changes_error(git_changes::ChangesError::new(
                git_changes::codes::NOT_FOUND,
                format!("{ws} is not a K2 workspace on this server"),
            ))
        }
    };
    if !std::path::Path::new(&root).is_dir() {
        return changes_error(git_changes::ChangesError::new(
            git_changes::codes::NOT_FOUND,
            format!("{root} is not a folder on this server"),
        ));
    }
    if let Some((_, existing)) = git_changes::repo_key_and_root(&root) {
        return CliResponse {
            status: "409 Conflict",
            content_type: "application/json",
            body: serde_json::json!({
                "error": format!("{root} is already in a git repository ({existing})"),
                "code": "already_a_repo",
            })
            .to_string(),
        };
    }
    if let Err(e) = git::init_repo(&root) {
        return CliResponse::internal_error(e);
    }
    crate::fs_live::resync_watches();
    let rev = crate::git_live::changed_now(&root);
    ok_serialized(serde_json::json!({ "success": true, "repoRoot": root, "rev": rev }))
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
    if flag_param(params, "sides") {
        return handle_git_diff_sides(&path, &file_path, params);
    }
    serialized(git::diff_file(&path, &file_path))
}

/// Changes tab v1 — `diff-file?sides=1`: both full sides (prd-changes-tab-v1 §2).
fn handle_git_diff_sides(path: &str, file_path: &str, params: &HashMap<String, String>) -> CliResponse {
    let mode = match git_changes::DiffMode::parse(
        params.get("mode").map(String::as_str),
        flag_param(params, "staged"),
        params.get("base").map(String::as_str),
    ) {
        Ok(m) => m,
        Err(e) => return changes_error(e),
    };
    let old_path = params.get("old_path").map(String::as_str);
    match git_changes::diff_sides(path, file_path, old_path, mode) {
        Ok(d) => ok_serialized(d),
        Err(e) => changes_error(e),
    }
}

/// Changes tab v1 — `GET /cli/git/snapshot` (prd-changes-tab-v1 §1).
pub fn handle_git_snapshot(params: &HashMap<String, String>) -> CliResponse {
    let path = str_param(params, "path");
    if path.is_empty() {
        return changes_error(git_changes::ChangesError::new(
            git_changes::codes::BAD_REQUEST,
            "Missing 'path' parameter",
        ));
    }
    match git_changes::snapshot(&path, params.get("base").map(String::as_str)) {
        Ok(s) => ok_serialized(s),
        Err(e) => changes_error(e),
    }
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

    // Changes tab: watch the new checkout's git dir too.
    crate::fs_live::resync_watches();
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
    let rel = match checked_rel(&b.path, &b.file_path) {
        Ok(r) => r,
        Err(r) => return r,
    };
    changed_ok(&b.path, git::stage_file(&b.path, &rel))
}

pub fn handle_git_unstage(body: &[u8]) -> CliResponse {
    let b: StageBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let rel = match checked_rel(&b.path, &b.file_path) {
        Ok(r) => r,
        Err(r) => return r,
    };
    changed_ok(&b.path, git::unstage_file(&b.path, &rel))
}

/// `unit_ok`, plus an immediate `git_changed` for the caller's workspace
/// when the op succeeded (the watcher would catch it ~300 ms later).
fn changed_ok(path: &str, r: Result<(), String>) -> CliResponse {
    if r.is_ok() {
        crate::git_live::changed_now(path);
    }
    unit_ok(r)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiscardBody {
    path: String,
    files: Vec<String>,
    #[serde(default)]
    include_staged: bool,
}

#[derive(Deserialize)]
struct OpenChangesBody {
    #[serde(default)]
    path: String,
}

/// Changes tab (C5, `k2 changes open`) — `POST /cli/git/open-changes`
/// `{path}`: emit the app-level `open_changes {workspacePath}`; a window
/// whose room shows that workspace opens (or focuses) its Changes tab.
/// The daemon can't tell whether any window is listening, so `200` only
/// means "asked".
pub fn handle_git_open_changes(is_post: bool, body: &[u8]) -> CliResponse {
    if !is_post {
        return CliResponse::method_not_allowed();
    }
    let b: OpenChangesBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let t = b.path.trim();
    let ws = if t.len() > 1 { t.trim_end_matches('/') } else { t };
    if ws.is_empty() {
        return changes_error(git_changes::ChangesError::new(git_changes::codes::BAD_REQUEST, "Missing 'path'"));
    }
    if !std::path::Path::new(ws).is_dir() {
        return changes_error(git_changes::ChangesError::new(
            git_changes::codes::NOT_FOUND,
            format!("{ws} is not a folder on this server"),
        ));
    }
    crate::session_events::emit_open_changes(ws);
    ok_serialized(serde_json::json!({ "success": true, "workspacePath": ws }))
}

/// Changes tab v1 — `POST /cli/git/discard` (prd-changes-tab-v1 §3).
/// Untracked files and every working copy that would be lost go to the
/// Trash (`K2_GIT_TRASH_DIR` in tests/smoke); nothing is unlinked.
pub fn handle_git_discard(is_post: bool, body: &[u8]) -> CliResponse {
    if !is_post {
        return CliResponse::method_not_allowed();
    }
    let b: DiscardBody = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if b.path.trim().is_empty() {
        return changes_error(git_changes::ChangesError::new(
            git_changes::codes::BAD_REQUEST,
            "Missing 'path'",
        ));
    }
    match git_changes::discard(&b.path, &b.files, b.include_staged, &git_changes::trash_path) {
        Ok(out) => {
            let did_something = !out.restored.is_empty() || !out.trashed.is_empty() || !out.unstaged.is_empty();
            let rev = if did_something {
                crate::git_live::changed_now(&b.path).unwrap_or_else(|| git_changes::current_rev(&out.rev_key))
            } else {
                git_changes::current_rev(&out.rev_key)
            };
            ok_serialized(serde_json::json!({
                "success": true,
                "restored": out.restored,
                "trashed": out.trashed,
                "unstaged": out.unstaged,
                "skipped": out.skipped,
                "failed": out.failed,
                "rev": rev,
            }))
        }
        Err(e) => changes_error(e),
    }
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
    changed_ok(&b.path, git::stage_all(&b.path))
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
    let r = git::commit(&b.path, &b.message);
    if r.is_ok() {
        crate::git_live::changed_now(&b.path);
    }
    serialized(r)
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
// Agent passports (cell socket) — `k2 changes`
// ══════════════════════════════════════════════════════════════════════

/// The Changes routes an agent passport may call on its cell socket,
/// scoped to its own workspace (prd-changes-tab-v1 "Agent access").
pub const AGENT_ROUTES: &[&str] = &[
    "/cli/git/commit",
    "/cli/git/diff-file",
    "/cli/git/discard",
    // C5 scope add: `k2 changes init` on the agent's own workspace root.
    "/cli/git/init",
    // C5: read the review notes left for you, resolve them when fixed.
    "/cli/git/notes",
    "/cli/git/notes/resolve",
    // C5: `k2 changes open` — ask the human's window to show the tab.
    "/cli/git/open-changes",
    "/cli/git/snapshot",
    "/cli/git/stage",
    "/cli/git/stage-all",
    "/cli/git/unstage",
];

const AGENT_POST_ROUTES: &[&str] = &[
    "/cli/git/commit",
    "/cli/git/discard",
    "/cli/git/init",
    "/cli/git/notes/resolve",
    "/cli/git/open-changes",
    "/cli/git/stage",
    "/cli/git/stage-all",
    "/cli/git/unstage",
];

pub fn is_agent_route(path: &str) -> bool {
    AGENT_ROUTES.contains(&path)
}

fn agent_forbidden(hint: String) -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({ "error": hint, "code": "forbidden" }).to_string(),
    }
}

/// The repo path an agent asked for, when it (and its repo root) are
/// inside the agent's own workspace. Empty → the workspace itself.
pub(crate) fn agent_scope(requested: &str, own_workspace: &str) -> Result<String, CliResponse> {
    let own = own_workspace.trim_end_matches('/');
    let want = if requested.trim().is_empty() { own.to_string() } else { requested.trim_end_matches('/').to_string() };
    let canon = |p: &str| std::fs::canonicalize(p).ok();
    let (Some(own_c), Some(want_c)) = (canon(own), canon(&want)) else {
        return Err(agent_forbidden(format!("'{want}' is not a folder in your workspace ({own})")));
    };
    if !want_c.starts_with(&own_c) {
        return Err(agent_forbidden(format!(
            "agents can only use the Changes routes on their own workspace ({own}); '{want}' is outside it"
        )));
    }
    // The repo must not reach above the workspace (a workspace that is a
    // sub-folder of a bigger repo would let discard touch files outside it).
    if let Some((_, root)) = git_changes::repo_key_and_root(&want) {
        let inside = canon(&root).is_some_and(|r| r.starts_with(&own_c));
        if !inside {
            return Err(agent_forbidden(format!(
                "the git repository at {root} reaches outside your workspace ({own})"
            )));
        }
    }
    Ok(want)
}

/// Serve one [`AGENT_ROUTES`] call for a passport whose workspace is
/// `own_workspace` (resolved from the passport, never the request). GETs
/// read `path` from the query, POSTs from the JSON body; a missing
/// `path` means the agent's own workspace.
pub fn handle_agent_route(
    path: &str,
    is_post: bool,
    params: &HashMap<String, String>,
    body: &[u8],
    own_workspace: &str,
) -> CliResponse {
    handle_agent_route_as(path, is_post, params, body, own_workspace, "agent")
}

/// [`handle_agent_route`] with the passport's actor label (`agent:<handle>`,
/// recorded as `resolved_by` when the agent resolves a review note).
pub fn handle_agent_route_as(
    path: &str,
    is_post: bool,
    params: &HashMap<String, String>,
    body: &[u8],
    own_workspace: &str,
    agent_actor: &str,
) -> CliResponse {
    if !is_agent_route(path) {
        return CliResponse::not_found();
    }
    let post_route = AGENT_POST_ROUTES.contains(&path);
    if post_route != is_post {
        return CliResponse::method_not_allowed();
    }
    // C5 review notes: their own handler, scoped per note.
    if crate::git_notes_routes::is_route(path) {
        let caller = crate::git_notes_routes::Caller::Agent {
            actor: agent_actor.to_string(),
            own_workspace: own_workspace.to_string(),
        };
        if !is_post {
            return crate::git_notes_routes::handle_list(params, &caller);
        }
        return crate::git_notes_routes::dispatch_post(path, body, &caller, &crate::git_notes_routes::deliver_to_agent);
    }
    if post_route {
        let mut v: serde_json::Value = if body.iter().all(u8::is_ascii_whitespace) {
            serde_json::json!({})
        } else {
            match serde_json::from_slice(body) {
                Ok(v) => v,
                Err(e) => return CliResponse::bad_request(format!("invalid JSON: {e}")),
            }
        };
        let Some(obj) = v.as_object_mut() else {
            return CliResponse::bad_request("body must be a JSON object");
        };
        let requested = obj.get("path").and_then(|p| p.as_str()).unwrap_or("").to_string();
        let scoped = match agent_scope(&requested, own_workspace) {
            Ok(p) => p,
            Err(r) => return r,
        };
        obj.insert("path".into(), serde_json::Value::String(scoped));
        return dispatch_unit4_git_post(path, v.to_string().as_bytes());
    }
    let mut p = params.clone();
    let requested = p.get("path").cloned().unwrap_or_default();
    let scoped = match agent_scope(&requested, own_workspace) {
        Ok(s) => s,
        Err(r) => return r,
    };
    p.insert("path".into(), scoped);
    match path {
        "/cli/git/snapshot" => handle_git_snapshot(&p),
        "/cli/git/diff-file" => handle_git_diff_file(&p),
        _ => CliResponse::not_found(),
    }
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
        "/cli/git/discard" => handle_git_discard(true, body),
        "/cli/git/open-changes" => handle_git_open_changes(true, body),
        "/cli/git/init" => handle_git_init(true, body),
        "/cli/git/merge-branch" => handle_git_merge_branch(body),
        "/cli/git/abort-merge" => handle_git_abort_merge(body),
        "/cli/git/resolve" => handle_git_resolve_conflict(body),
        "/cli/git/delete-branch" => handle_git_delete_branch(body),
        "/cli/git/prune-worktrees" => handle_git_prune_worktrees(body),
        _ => CliResponse::not_found(),
    }
}

#[cfg(test)]
#[path = "git_routes_tests.rs"]
mod tests;
